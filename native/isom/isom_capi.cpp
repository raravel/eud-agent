/* isom_capi.cpp - C ABI shim implementation over the vendored isom-poc engine.
 *
 * Design (import-then-extend, NOT a rewrite of the engine):
 *   The verified map operations already live in MapGenCli.cpp as `mapGenMain`
 *   subcommands ("chk", "locedit", "playeredit"). Those routines own the exact
 *   save flags the rules require -- locEdit/playerEdit save in place with
 *   save(path, overwriting=true, updateListFile=true, lockAnywhere=true,
 *   autoDefragmentLocations=false) and dumpChk writes the Remastered .chk.
 *   Rather than duplicate (and risk
 *   diverging from) that logic, this shim drives `mapGenMain` with a synthesized
 *   argv and marshals buffers <-> temp files.
 *
 * Boundary safety:
 *   - Every public function is wrapped so that no C++ exception and no structured
 *     (SEH) fault can unwind across the extern "C" boundary; both are converted
 *     to a nonzero IsomStatus.
 *   - Buffers handed back to Rust are allocated with std::malloc and released by
 *     isom_free (matching allocator).
 *
 * Raw-byte fidelity:
 *   - The `ops` buffer is written to the temp ops file byte-for-byte. Location
 *     NAME bytes therefore reach the engine's string pool exactly as Rust passed
 *     them -- no re-encode happens here (see rules.md).
 */
#include "isom_capi.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include <Windows.h>

/* mapGenMain has external linkage in MapGenCli.cpp (compiled into this lib).
 * It dispatches the "chk" / "locedit" / "playeredit" subcommands we drive here. */
int mapGenMain(int argc, char* argv[]);

namespace {

// RAII for a uniquely-named temp file path under %TEMP%. The file is deleted on
// destruction (best-effort). We only need the path; the engine does the open.
class TempFile
{
public:
    explicit TempFile(const char* suffix)
    {
        char dir[MAX_PATH] = {0};
        DWORD n = ::GetTempPathA(MAX_PATH, dir);
        if ( n == 0 || n > MAX_PATH )
            std::strcpy(dir, ".\\");
        char name[MAX_PATH] = {0};
        // Unique base name; suffix only documents intent (the engine keys off argv).
        if ( ::GetTempFileNameA(dir, "ism", 0, name) == 0 )
        {
            path_.clear();
            return;
        }
        path_ = name;
        (void)suffix;
    }
    ~TempFile()
    {
        if ( !path_.empty() )
            ::DeleteFileA(path_.c_str());
    }
    const std::string& path() const { return path_; }
    bool valid() const { return !path_.empty(); }

    TempFile(const TempFile&) = delete;
    TempFile& operator=(const TempFile&) = delete;

private:
    std::string path_;
};

// Write raw bytes to a path with no transformation (binary, no BOM, no newline
// translation). Returns false on any I/O failure.
bool writeAllBytes(const std::string& path, const uint8_t* data, size_t len)
{
    HANDLE h = ::CreateFileA(path.c_str(), GENERIC_WRITE, 0, nullptr,
                             CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if ( h == INVALID_HANDLE_VALUE )
        return false;
    bool ok = true;
    size_t off = 0;
    while ( ok && off < len )
    {
        DWORD chunk = (len - off > 0x10000000u) ? 0x10000000u : DWORD(len - off);
        DWORD wrote = 0;
        if ( !::WriteFile(h, data + off, chunk, &wrote, nullptr) || wrote == 0 )
            ok = false;
        else
            off += wrote;
    }
    ::CloseHandle(h);
    return ok && off == len;
}

// Read an entire file into a malloc'd buffer (caller frees via isom_free).
// Returns ISOM_OK / ISOM_ERR_IO. On success *out/*outLen are set.
int readAllBytes(const std::string& path, uint8_t** out, size_t* outLen)
{
    HANDLE h = ::CreateFileA(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                             OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if ( h == INVALID_HANDLE_VALUE )
        return ISOM_ERR_IO;

    LARGE_INTEGER size {};
    if ( !::GetFileSizeEx(h, &size) || size.QuadPart < 0 )
    {
        ::CloseHandle(h);
        return ISOM_ERR_IO;
    }
    size_t len = size_t(size.QuadPart);
    // malloc(0) may return NULL; allocate at least 1 byte so out is non-null.
    uint8_t* buf = static_cast<uint8_t*>(std::malloc(len ? len : 1));
    if ( buf == nullptr )
    {
        ::CloseHandle(h);
        return ISOM_ERR_IO;
    }
    size_t off = 0;
    bool ok = true;
    while ( ok && off < len )
    {
        DWORD chunk = (len - off > 0x10000000u) ? 0x10000000u : DWORD(len - off);
        DWORD got = 0;
        if ( !::ReadFile(h, buf + off, chunk, &got, nullptr) || got == 0 )
            ok = false;
        else
            off += got;
    }
    ::CloseHandle(h);
    if ( !ok || off != len )
    {
        std::free(buf);
        return ISOM_ERR_IO;
    }
    *out = buf;
    *outLen = len;
    return ISOM_OK;
}

// Drive mapGenMain with a synthesized argv. argv strings are mutable copies (the
// engine takes char* argv[], C-main style). Returns the engine's int result.
int runMapGen(const std::vector<std::string>& args)
{
    std::vector<std::string> owned = args;
    std::vector<char*> argv;
    argv.reserve(owned.size() + 1);
    for ( auto& s : owned )
        argv.push_back(s.empty() ? const_cast<char*>("") : &s[0]);
    argv.push_back(nullptr);
    return mapGenMain(int(owned.size()), argv.data());
}

// Translate a SEH structured exception into our fault code. Used as the filter
// for the __try wrapper so access violations etc. don't escape the boundary.
int sehFilter(unsigned int /*code*/)
{
    return EXCEPTION_EXECUTE_HANDLER;
}

// SEH-guarded invocation of a C++ lambda. SEH and C++ EH cannot share one frame
// under /EHsc, so the C++ try/catch lives one frame out (in the public funcs)
// and this frame catches only structured faults.
template <typename Fn>
int guardSeh(Fn&& fn, int& engineResultOut)
{
    __try
    {
        engineResultOut = fn();
        return ISOM_OK;
    }
    __except ( sehFilter(::GetExceptionCode()) )
    {
        return ISOM_ERR_FAULT;
    }
}

// Shared body for the two ops-based editors. cmd is "locedit" or "playeredit".
int applyOps(const char* cmd, const char* mapPath,
             const uint8_t* ops, size_t opsLen)
{
    if ( mapPath == nullptr || mapPath[0] == '\0' )
        return ISOM_ERR_INVALID_ARG;
    if ( ops == nullptr || opsLen == 0 )
        return ISOM_ERR_INVALID_ARG;

    TempFile opsFile(".ops");
    if ( !opsFile.valid() )
        return ISOM_ERR_IO;
    // RAW pass-through: the op program (incl. location NAME bytes) is written
    // verbatim; no re-encode happens in the shim.
    if ( !writeAllBytes(opsFile.path(), ops, opsLen) )
        return ISOM_ERR_IO;

    const std::vector<std::string> args { "isom", cmd, mapPath, opsFile.path() };
    int engineResult = 1;
    int guard = guardSeh([&]() { return runMapGen(args); }, engineResult);
    if ( guard != ISOM_OK )
        return guard;
    return engineResult == 0 ? ISOM_OK : ISOM_ERR_ENGINE;
}

} // namespace

extern "C" {

int isom_abi_version(void)
{
    return ISOM_ABI_VERSION;
}

int isom_chk_extract(const char* map_path, uint8_t** out, size_t* out_len)
{
    if ( out == nullptr || out_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out = nullptr;
    *out_len = 0;
    if ( map_path == nullptr || map_path[0] == '\0' )
        return ISOM_ERR_INVALID_ARG;

    try
    {
        TempFile chkFile(".chk");
        if ( !chkFile.valid() )
            return ISOM_ERR_IO;

        // mapGenMain "chk" <map> <out.chk> writes the Remastered .chk to a file.
        const std::vector<std::string> args { "isom", "chk", map_path, chkFile.path() };
        int engineResult = 1;
        int guard = guardSeh([&]() { return runMapGen(args); }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_OPEN_MAP; // dumpChk fails on open or save

        return readAllBytes(chkFile.path(), out, out_len);
    }
    catch ( ... )
    {
        if ( *out ) { std::free(*out); *out = nullptr; *out_len = 0; }
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_locedit(const char* map_path, const uint8_t* ops, size_t ops_len)
{
    try
    {
        return applyOps("locedit", map_path, ops, ops_len);
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_playeredit(const char* map_path, const uint8_t* ops, size_t ops_len)
{
    try
    {
        return applyOps("playeredit", map_path, ops, ops_len);
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

void isom_free(uint8_t* p)
{
    std::free(p);
}

} // extern "C"

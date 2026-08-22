/* isom_capi.cpp - C ABI shim implementation over the vendored isom-poc engine.
 *
 * Design (import-then-extend, NOT a rewrite of the engine):
 *   The verified map operations already live in MapGenCli.cpp as `mapGenMain`
 *   subcommands ("chk", "locedit", "playeredit", "switchedit", "render").
 *   Those routines own the exact save flags the rules require -- every in-place
 *   editor saves with lockAnywhere=true and autoDefragmentLocations=false, while
 *   dumpChk/render write extracted artifacts.
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
#include "IsomTerrain/MapAgentCore.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include <Windows.h>

/* mapGenMain has external linkage in MapGenCli.cpp (compiled into this lib).
 * It dispatches the commands this shim drives. */
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

// C++ exceptions use the MSVC 0xE06D7363 SEH code while unwinding through
// this frame. Let those continue to the outer C++ catch; handle only actual
// structured faults such as access violations here.
int sehFilter(unsigned int code)
{
    constexpr unsigned int MsvcCppException = 0xE06D7363u;
    return code == MsvcCppException ? EXCEPTION_CONTINUE_SEARCH : EXCEPTION_EXECUTE_HANDLER;
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

// Shared body for the ops-based map editors.
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
int copyBuffer(const void* data, size_t length, uint8_t** out, size_t* outLength)
{
    uint8_t* buffer = static_cast<uint8_t*>(std::malloc(length == 0 ? 1 : length));
    if ( buffer == nullptr )
        return ISOM_ERR_IO;
    if ( length != 0 )
        std::memcpy(buffer, data, length);
    *out = buffer;
    *outLength = length;
    return ISOM_OK;
}

int copyString(const std::string& value, uint8_t** out, size_t* outLength)
{
    return copyBuffer(value.data(), value.size(), out, outLength);
}

std::string errorReport(const std::string& message)
{
    std::string escaped;
    for ( unsigned char value : message )
    {
        switch ( value )
        {
        case '\"': escaped += "\\\""; break;
        case '\\': escaped += "\\\\"; break;
        case '\n': escaped += "\\n"; break;
        case '\r': escaped += "\\r"; break;
        case '\t': escaped += "\\t"; break;
        default:
            if ( value >= 0x20 )
                escaped.push_back(static_cast<char>(value));
        }
    }
    return "{\"schema\":\"eud-map-error/1\",\"ok\":false,\"error\":\"" + escaped + "\"}";
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

int isom_switchedit(const char* map_path, const uint8_t* ops, size_t ops_len)
{
    try
    {
        return applyOps("switchedit", map_path, ops, ops_len);
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_render_map(const char* map_path, const char* starcraft_path,
                    uint32_t scale, uint8_t** out, size_t* out_len)
{
    if ( out == nullptr || out_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out = nullptr;
    *out_len = 0;
    if ( map_path == nullptr || map_path[0] == '\0'
         || starcraft_path == nullptr || starcraft_path[0] == '\0'
         || (scale != 1 && scale != 2 && scale != 4 && scale != 8) )
        return ISOM_ERR_INVALID_ARG;

    try
    {
        TempFile bmpFile(".bmp");
        if ( !bmpFile.valid() )
            return ISOM_ERR_IO;

        const std::vector<std::string> args {
            "isom", "render", map_path, bmpFile.path(),
            std::to_string(scale), starcraft_path
        };
        int engineResult = 1;
        int guard = guardSeh([&]() { return runMapGen(args); }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;

        return readAllBytes(bmpFile.path(), out, out_len);
    }
    catch ( ... )
    {
        if ( *out ) { std::free(*out); *out = nullptr; *out_len = 0; }
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_mapedit(
    const char* input_map_path,
    const char* output_map_path,
    const char* starcraft_path,
    const uint8_t* batch_json,
    size_t batch_len,
    uint8_t** out_report_json,
    size_t* out_report_len)
{
    if ( out_report_json == nullptr || out_report_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out_report_json = nullptr;
    *out_report_len = 0;
    if ( input_map_path == nullptr || input_map_path[0] == '\0'
         || output_map_path == nullptr || output_map_path[0] == '\0'
         || starcraft_path == nullptr || starcraft_path[0] == '\0'
         || batch_json == nullptr || batch_len == 0 )
        return ISOM_ERR_INVALID_ARG;
    try
    {
        std::string report;
        int engineResult = 1;
        const int guard = guardSeh([&]() {
            return mapagent::mapEdit(input_map_path, output_map_path, starcraft_path,
                batch_json, batch_len, report);
        }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;
        return copyString(report, out_report_json, out_report_len);
    }
    catch ( const std::exception& error )
    {
        const std::string report = errorReport(error.what());
        const int copied = copyString(report, out_report_json, out_report_len);
        return copied == ISOM_OK ? ISOM_ERR_ENGINE : copied;
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_render_region(
    const char* map_path,
    const char* starcraft_path,
    const uint8_t* request_json,
    size_t request_len,
    uint8_t** out_rgba,
    size_t* out_rgba_len,
    uint32_t* out_width,
    uint32_t* out_height)
{
    if ( out_rgba == nullptr || out_rgba_len == nullptr || out_width == nullptr || out_height == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out_rgba = nullptr;
    *out_rgba_len = 0;
    *out_width = 0;
    *out_height = 0;
    if ( map_path == nullptr || map_path[0] == '\0'
         || starcraft_path == nullptr || starcraft_path[0] == '\0'
         || request_json == nullptr || request_len == 0 )
        return ISOM_ERR_INVALID_ARG;
    try
    {
        std::vector<uint8_t> rgba;
        uint32_t width = 0;
        uint32_t height = 0;
        int engineResult = 1;
        const int guard = guardSeh([&]() {
            return mapagent::renderRegion(map_path, starcraft_path, request_json, request_len, rgba, width, height);
        }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;
        const int copied = copyBuffer(rgba.data(), rgba.size(), out_rgba, out_rgba_len);
        if ( copied != ISOM_OK )
            return copied;
        *out_width = width;
        *out_height = height;
        return ISOM_OK;
    }
    catch ( const std::exception& error )
    {
        if ( *out_rgba != nullptr )
        {
            std::free(*out_rgba);
            *out_rgba = nullptr;
            *out_rgba_len = 0;
        }
        const std::string report = errorReport(error.what());
        const int copied = copyString(report, out_rgba, out_rgba_len);
        return copied == ISOM_OK ? ISOM_ERR_ENGINE : copied;
    }
    catch ( ... )
    {
        if ( *out_rgba != nullptr )
        {
            std::free(*out_rgba);
            *out_rgba = nullptr;
            *out_rgba_len = 0;
        }
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_catalog_query(
    const char* starcraft_path,
    const uint8_t* request_json,
    size_t request_len,
    uint8_t** out_json,
    size_t* out_json_len)
{
    if ( out_json == nullptr || out_json_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out_json = nullptr;
    *out_json_len = 0;
    if ( starcraft_path == nullptr || starcraft_path[0] == '\0'
         || request_json == nullptr || request_len == 0 )
        return ISOM_ERR_INVALID_ARG;
    try
    {
        std::string result;
        int engineResult = 1;
        const int guard = guardSeh([&]() {
            return mapagent::catalogQuery(starcraft_path, request_json, request_len, result);
        }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;
        return copyString(result, out_json, out_json_len);
    }
    catch ( const std::exception& error )
    {
        const std::string report = errorReport(error.what());
        const int copied = copyString(report, out_json, out_json_len);
        return copied == ISOM_OK ? ISOM_ERR_ENGINE : copied;
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_image_quantize(
    const char* starcraft_path,
    uint16_t tileset,
    const uint8_t* rgba,
    size_t rgba_len,
    uint16_t width,
    uint16_t height,
    const uint16_t* before_tiles,
    size_t before_tile_count,
    uint8_t** out_result,
    size_t* out_result_len)
{
    if ( out_result == nullptr || out_result_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out_result = nullptr;
    *out_result_len = 0;
    if ( starcraft_path == nullptr || starcraft_path[0] == '\0'
         || rgba == nullptr || before_tiles == nullptr )
        return ISOM_ERR_INVALID_ARG;
    try
    {
        std::vector<std::uint8_t> result;
        int engineResult = 1;
        const int guard = guardSeh([&]() {
            return mapagent::imageQuantize(
                starcraft_path,
                tileset,
                rgba,
                rgba_len,
                width,
                height,
                before_tiles,
                before_tile_count,
                result);
        }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;
        return copyBuffer(result.data(), result.size(), out_result, out_result_len);
    }
    catch ( const std::exception& error )
    {
        const std::string report = errorReport(error.what());
        const int copied = copyString(report, out_result, out_result_len);
        return copied == ISOM_OK ? ISOM_ERR_ENGINE : copied;
    }
    catch ( ... )
    {
        return ISOM_ERR_EXCEPTION;
    }
}

int isom_map_digest(const char* map_path, uint8_t** out_json, size_t* out_json_len)
{
    if ( out_json == nullptr || out_json_len == nullptr )
        return ISOM_ERR_INVALID_ARG;
    *out_json = nullptr;
    *out_json_len = 0;
    if ( map_path == nullptr || map_path[0] == '\0' )
        return ISOM_ERR_INVALID_ARG;
    try
    {
        std::string result;
        int engineResult = 1;
        const int guard = guardSeh([&]() {
            return mapagent::mapDigest(map_path, result);
        }, engineResult);
        if ( guard != ISOM_OK )
            return guard;
        if ( engineResult != 0 )
            return ISOM_ERR_ENGINE;
        return copyString(result, out_json, out_json_len);
    }
    catch ( const std::exception& error )
    {
        const std::string report = errorReport(error.what());
        const int copied = copyString(report, out_json, out_json_len);
        return copied == ISOM_OK ? ISOM_ERR_ENGINE : copied;
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

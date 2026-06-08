/* isom_capi.h - C ABI shim over the vendored isom-poc map engine.
 *
 * This is the ONLY surface the Rust `isom-sys` crate binds to (via bindgen).
 * It is plain C: no C++ types, no STL, and no exceptions cross this boundary.
 * Every C++ exception (and SEH fault) raised inside the engine is caught at the
 * shim and converted into a nonzero error code; nothing is allowed to unwind out
 * of an `extern "C"` function.
 *
 * Buffers returned through out-params are heap-allocated by the isom_* functions
 * and MUST be released by the caller with isom_free().
 *
 * The shim routes into the verified isom-poc code paths (MapGenCli `mapGenMain`:
 * chk / locedit / playeredit). The in-place map save keeps
 * autoDefragmentLocations=false and lockAnywhere=true (see rules.md). Location
 * NAME bytes inside `ops` are passed through as RAW bytes and are never
 * re-encoded here.
 */
#ifndef ISOM_CAPI_H
#define ISOM_CAPI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version of this shim. Bump on any breaking change to the signatures or
 * the ops/buffer encoding below. The Rust side asserts this at startup. */
#define ISOM_ABI_VERSION 1

/* Error codes returned by the isom_* functions. 0 == success. */
enum IsomStatus {
    ISOM_OK = 0,
    ISOM_ERR_INVALID_ARG = 1, /* null pointer / empty path / bad length      */
    ISOM_ERR_OPEN_MAP = 2,    /* map could not be opened or is empty         */
    ISOM_ERR_IO = 3,          /* temp-file read/write or other I/O failure   */
    ISOM_ERR_ENGINE = 4,      /* engine returned a nonzero (op/save) failure */
    ISOM_ERR_EXCEPTION = 5,   /* a C++ exception was caught at the shim      */
    ISOM_ERR_FAULT = 6        /* a structured (SEH) fault was caught         */
};

/* Returns ISOM_ABI_VERSION. Used by Rust as a load-time sanity check that the
 * linked static lib matches the bindings. */
int isom_abi_version(void);

/* Extract the raw CHK (Remastered .chk) from a map (.scx/.scm) into a freshly
 * allocated buffer.
 *   map_path : UTF-8, NUL-terminated path to the source map.
 *   out      : receives a malloc'd buffer with the CHK bytes (free w/ isom_free).
 *   out_len  : receives the buffer length in bytes.
 * Returns 0 on success, nonzero IsomStatus otherwise. On failure *out is NULL. */
int isom_chk_extract(const char* map_path, uint8_t** out, size_t* out_len);

/* Apply a batch of MRGN location ops to an existing map, saved IN PLACE.
 *   map_path : UTF-8, NUL-terminated path to the map to edit in place.
 *   ops      : RAW bytes of the op program (one pipe-separated op per line;
 *              see MapGenCli locEdit). Location NAME bytes are passed through
 *              verbatim -- NEVER re-encoded here.
 *   ops_len  : length of `ops` in bytes.
 * All-or-nothing: any invalid op aborts BEFORE the save. The save keeps
 * autoDefragmentLocations=false and lockAnywhere=true (location ids never
 * renumber; #64 Anywhere is protected). Returns 0 on success. */
int isom_locedit(const char* map_path, const uint8_t* ops, size_t ops_len);

/* Apply a batch of player ops (start locations + OWNR controllers) to an
 * existing map, saved IN PLACE. Same buffer/encoding/safety contract as
 * isom_locedit (autoDefragmentLocations=false on save). Returns 0 on success. */
int isom_playeredit(const char* map_path, const uint8_t* ops, size_t ops_len);

/* Free a buffer previously returned by an isom_* function. Safe on NULL. */
void isom_free(uint8_t* p);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* ISOM_CAPI_H */

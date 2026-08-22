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
 * chk / locedit / playeredit / switchedit / render). In-place map saves keep
 * autoDefragmentLocations=false and lockAnywhere=true (see rules.md). Map-text
 * bytes inside `ops` are passed through as RAW bytes and are never re-encoded
 * here.
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
#define ISOM_ABI_VERSION 5

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

/* Rename switches in an existing map, saved IN PLACE. Ops are
 * `rename|<1-based id>|<raw name bytes>`. Trigger references are numeric and
 * remain unchanged. Same all-or-nothing and save-safety contract as locedit. */
int isom_switchedit(const char* map_path, const uint8_t* ops, size_t ops_len);

/* Render the map terrain through the verified VR4/VX4/WPE renderer.
 * Returns a malloc'd 24-bpp BMP buffer (free with isom_free). `scale` must be
 * 1, 2, 4, or 8. Paths are UTF-8 and NUL-terminated. */
int isom_render_map(const char* map_path, const char* starcraft_path,
                    uint32_t scale, uint8_t** out, size_t* out_len);
/* Apply one strict eud-map-edit/1 JSON batch to an existing map. The input is
 * loaded once, every operation is applied in memory, and output is promoted
 * only after one successful save and native re-open verification. Input and
 * output paths must differ. The report buffer is returned on success and may
 * also contain a structured error on failure. */
int isom_mapedit(
    const char* input_map_path,
    const char* output_map_path,
    const char* starcraft_path,
    const uint8_t* batch_json,
    size_t batch_len,
    uint8_t** out_report_json,
    size_t* out_report_len);

/* Render a strict eud-map-render/1 region or palette thumbnail as top-down RGBA.
 * On a standard C++ validation/engine error, out_rgba contains an
 * eud-map-error/1 JSON report instead of pixels so the caller can surface the
 * actionable native message. Every returned buffer is freed with isom_free(). */
int isom_render_region(
    const char* map_path,
    const char* starcraft_path,
    const uint8_t* request_json,
    size_t request_len,
    uint8_t** out_rgba,
    size_t* out_rgba_len,
    uint32_t* out_width,
    uint32_t* out_height);

/* Query semantic brushes, exact tiles, and actual DAT/GRP-backed object catalogs. */
int isom_catalog_query(
    const char* starcraft_path,
    const uint8_t* request_json,
    size_t request_len,
    uint8_t** out_json,
    size_t* out_json_len);

/* Return the file/container digest including named extra MPQ asset hashes. */
int isom_map_digest(
    const char* map_path,
    uint8_t** out_json,
    size_t* out_json_len);

/* Quantize one bounded RGBA pixel per output map tile against the current
 * tileset's graphics-valid SD representative-color palette. `before_tiles`
 * supplies candidate terrain for alpha preservation/compositing. The returned
 * eud-map-image-quantize/1 binary is:
 *   magic "MIQ1", width u16 LE, height u16 LE,
 *   unique/walkability-changed/height-changed u32 LE,
 *   width*height tile ids u16 LE, then one RGB triplet per tile.
 * Width/height are 1..256 and every input/output allocation is bounded. */
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
    size_t* out_result_len);

/* Free a buffer previously returned by an isom_* function. Safe on NULL. */
void isom_free(uint8_t* p);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* ISOM_CAPI_H */

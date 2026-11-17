#ifndef CLIPRUST_H
#define CLIPRUST_H

#include <stddef.h>
#include <stdint.h>

/* C ABI of the Rust core (src/ffi.rs). Requests and responses are JSON. */

typedef struct CrHandle CrHandle;
typedef void (*cr_event_cb)(const char *event_json, void *ctx);

/* NULL db_path means the default location shared with the CLI. */
CrHandle *cr_open(const char *db_path, char **err_out);
void cr_close(CrHandle *h);

/* {"method": ..., "params": {...}} -> {"ok": ...} or {"err": "..."}; free with cr_free_string. */
char *cr_call(const CrHandle *h, const char *request_json);

/* 0 = ok, 1 = absent, 2 = error; free with cr_free_bytes. */
int32_t cr_item_data(const CrHandle *h, int64_t id, const char *format, uint8_t **out, size_t *len);

/* Starts the clipboard watcher thread once; cb runs on that thread. */
void cr_watch(const CrHandle *h, cr_event_cb cb, void *ctx);

void cr_free_string(char *s);
void cr_free_bytes(uint8_t *p, size_t len);

#endif

#ifndef CLIPRUST_H
#define CLIPRUST_H

#include <stddef.h>
#include <stdint.h>

/* C ABI of the Rust core (src/ffi). Requests and responses are JSON. */

typedef struct CrHandle CrHandle;
typedef void (*cr_event_cb)(const char *event_json, void *ctx);

/* NULL db_path means the default location shared with the CLI. On failure *err_out is
   {"code": ..., "message": ...}; code locked means cr_restore_key may help. */
CrHandle *cr_open(const char *db_path, char **err_out);

/* Puts a master key from a recovery code back into the Keychain, also while cr_open
   fails on an encrypted database. Returns its generation, or -1 and sets *err_out as above. */
int64_t cr_restore_key(const char *db_path, const char *code, char **err_out);
void cr_close(CrHandle *h);

/* {"method": ..., "params": {...}} -> {"ok": ...} or {"err": {"code": ..., "message": ...}};
   code is not_found, invalid, locked, io or internal. Free with cr_free_string. */
char *cr_call(const CrHandle *h, const char *request_json);

/* 0 = ok, 1 = absent, 2 = error; free with cr_free_bytes. */
int32_t cr_item_data(const CrHandle *h, int64_t id, const char *format, uint8_t **out, size_t *len);

/* Starts the clipboard watcher thread once; cb runs on that thread. */
void cr_watch(const CrHandle *h, cr_event_cb cb, void *ctx);

/* The app wrote pasteboard change `count` itself; the watcher skips it. */
void cr_own_change(intptr_t count);

/* The crate version, e.g. "0.1.0"; static, never freed. */
const char *cr_version(void);

/* Overwrites the string (it may hold a revealed key) before freeing it. */
void cr_free_string(char *s);
void cr_free_bytes(uint8_t *p, size_t len);

#endif

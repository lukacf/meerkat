#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    uint32_t job_id;
    bool background;
    bool ok;
    bool truncated;
    int status_code;
    size_t payload_len;
    char payload[2048];
} phase0_mpy_result_t;

esp_err_t phase0_mpy_init(void);

esp_err_t phase0_mpy_exec_sync(
    const char *code,
    uint32_t timeout_ms,
    phase0_mpy_result_t *out_result
);

esp_err_t phase0_mpy_submit_async(
    const char *code,
    uint32_t *out_job_id
);

esp_err_t phase0_mpy_poll_result(
    phase0_mpy_result_t *out_result
);

#ifdef __cplusplus
}
#endif

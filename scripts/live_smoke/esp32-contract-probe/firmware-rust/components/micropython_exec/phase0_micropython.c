#include "phase0_micropython.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_err.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_task.h"
#include "freertos/FreeRTOS.h"
#include "freertos/idf_additions.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "mbedtls/platform_time.h"
#include "modmachine.h"
#include "py/compile.h"
#include "py/gc.h"
#include "py/mphal.h"
#include "py/nlr.h"
#include "py/objmodule.h"
#include "py/qstr.h"
#include "py/runtime.h"
#include "py/stackctrl.h"
#include "py/misc.h"
#include "uart.h"
#include "usb.h"
#include "usb_serial_jtag.h"

#define PHASE0_MPY_QUEUE_DEPTH 4
#define PHASE0_MPY_STACK_BYTES (48 * 1024)
#define PHASE0_MPY_HEAP_BYTES (512 * 1024)
#define PHASE0_MPY_TIMEOUT_MS 15000

typedef struct {
    uint32_t job_id;
    bool background;
    char *code;
    phase0_mpy_result_t *sync_result;
    SemaphoreHandle_t sync_done;
} phase0_mpy_request_t;

static const char *TAG = "phase0-mpy";
static QueueHandle_t s_request_queue;
static QueueHandle_t s_result_queue;
static TaskHandle_t s_mpy_task;
static SemaphoreHandle_t s_init_done;
static esp_err_t s_init_result = ESP_FAIL;
static uint32_t s_next_job_id = 1;
static size_t s_mpy_stack_bytes = PHASE0_MPY_STACK_BYTES;
static bool s_mpy_stack_in_spiram = true;

extern void pthread_internal_local_storage_destructor_callback(TaskHandle_t handle);

void vPortCleanUpTCB(void *pxTCB) {
    pthread_internal_local_storage_destructor_callback((TaskHandle_t)pxTCB);
}

void usb_init(void) {
}

void usb_tx_strn(const char *str, size_t len) {
    (void)str;
    (void)len;
}

void machine_bitstream_high_low(
    mp_hal_pin_obj_t pin,
    uint32_t *timing_ns,
    const uint8_t *buf,
    size_t len
) {
    (void)pin;
    (void)timing_ns;
    (void)buf;
    (void)len;
}

static const char *WRAPPER_SRC =
    "_mp_out = []\n"
    "_mp_result = None\n"
    "_mp_error = ''\n"
    "globals()['result'] = None\n"
    "def print(*args, **kwargs):\n"
    "    sep = kwargs.get('sep', ' ')\n"
    "    end = kwargs.get('end', '\\n')\n"
    "    _mp_out.append(sep.join(str(a) for a in args) + end)\n"
    "try:\n"
    "    try:\n"
    "        _mp_result = eval(__tool_code, globals())\n"
    "        globals()['result'] = _mp_result\n"
    "    except SyntaxError:\n"
    "        exec(__tool_code, globals())\n"
    "        if 'result' in globals():\n"
    "            _mp_result = globals()['result']\n"
    "except Exception as e:\n"
    "    _mp_error = repr(e)\n"
    "try:\n"
    "    __tool_result = repr(_mp_result)\n"
    "except Exception as e:\n"
    "    __tool_result = '<repr failed: %s>' % e\n"
    "__tool_stdout = ''.join(_mp_out)\n"
    "__tool_stderr = ''\n"
    "__tool_error = _mp_error\n";

static void phase0_fill_result(
    phase0_mpy_result_t *out_result,
    uint32_t job_id,
    bool background,
    bool ok,
    int status_code,
    const char *payload
);

static const char *phase0_opt_str(mp_obj_dict_t *globals, const char *name, size_t *out_len) {
    if (out_len) {
        *out_len = 0;
    }
    if (!globals || !name) {
        return NULL;
    }
    mp_map_elem_t *elem = mp_map_lookup(
        &globals->map,
        MP_OBJ_NEW_QSTR(qstr_from_str(name)),
        MP_MAP_LOOKUP
    );
    if (!elem || elem->value == MP_OBJ_NULL || elem->value == mp_const_none) {
        return NULL;
    }
    size_t len = 0;
    const char *value = mp_obj_str_get_data(elem->value, &len);
    if (out_len) {
        *out_len = len;
    }
    return value;
}

static size_t phase0_json_escape(char *dst, size_t dst_size, const char *src, size_t src_len) {
    size_t written = 0;
    if (!dst || dst_size == 0) {
        return 0;
    }
    for (size_t i = 0; i < src_len && written + 1 < dst_size; ++i) {
        unsigned char ch = (unsigned char)src[i];
        const char *esc = NULL;
        switch (ch) {
            case '\\':
                esc = "\\\\";
                break;
            case '"':
                esc = "\\\"";
                break;
            case '\n':
                esc = "\\n";
                break;
            case '\r':
                esc = "\\r";
                break;
            case '\t':
                esc = "\\t";
                break;
            default:
                break;
        }
        if (esc) {
            size_t esc_len = strlen(esc);
            if (written + esc_len >= dst_size) {
                break;
            }
            memcpy(dst + written, esc, esc_len);
            written += esc_len;
            continue;
        }
        if (ch < 0x20) {
            if (written + 6 >= dst_size) {
                break;
            }
            written += (size_t)snprintf(dst + written, dst_size - written, "\\u%04x", ch);
            continue;
        }
        dst[written++] = (char)ch;
    }
    dst[written] = '\0';
    return written;
}

static void phase0_fill_payload_json(
    phase0_mpy_result_t *out_result,
    uint32_t job_id,
    bool background,
    bool ok,
    int status_code,
    const char *stdout_text,
    size_t stdout_len,
    const char *stderr_text,
    size_t stderr_len,
    const char *result_text,
    size_t result_len,
    const char *error_text,
    size_t error_len
) {
    char stdout_buf[1024];
    char stderr_buf[512];
    char result_buf[512];
    char error_buf[512];
    char payload[3072];

    phase0_json_escape(stdout_buf, sizeof(stdout_buf), stdout_text ? stdout_text : "", stdout_text ? stdout_len : 0);
    phase0_json_escape(stderr_buf, sizeof(stderr_buf), stderr_text ? stderr_text : "", stderr_text ? stderr_len : 0);
    phase0_json_escape(result_buf, sizeof(result_buf), result_text ? result_text : "", result_text ? result_len : 0);
    phase0_json_escape(error_buf, sizeof(error_buf), error_text ? error_text : "", error_text ? error_len : 0);

    if (error_text && error_len > 0) {
        snprintf(
            payload,
            sizeof(payload),
            "{\"stdout\":\"%s\",\"stderr\":\"%s\",\"result\":\"%s\",\"error\":\"%s\"}",
            stdout_buf,
            stderr_buf,
            result_buf,
            error_buf
        );
    } else {
        snprintf(
            payload,
            sizeof(payload),
            "{\"stdout\":\"%s\",\"stderr\":\"%s\",\"result\":\"%s\",\"error\":null}",
            stdout_buf,
            stderr_buf,
            result_buf
        );
    }

    phase0_fill_result(out_result, job_id, background, ok, status_code, payload);
}

static void phase0_fill_result(
    phase0_mpy_result_t *out_result,
    uint32_t job_id,
    bool background,
    bool ok,
    int status_code,
    const char *payload
) {
    if (!out_result) {
        return;
    }
    memset(out_result, 0, sizeof(*out_result));
    out_result->job_id = job_id;
    out_result->background = background;
    out_result->ok = ok;
    out_result->status_code = status_code;
    if (!payload) {
        return;
    }
    size_t payload_len = strlen(payload);
    size_t copy_len = payload_len;
    if (copy_len >= sizeof(out_result->payload)) {
        copy_len = sizeof(out_result->payload) - 1;
        out_result->truncated = true;
    }
    memcpy(out_result->payload, payload, copy_len);
    out_result->payload[copy_len] = '\0';
    out_result->payload_len = copy_len;
}

static bool phase0_exec_wrapper(const char *code, phase0_mpy_result_t *out_result) {
    bool ok = false;
    uint32_t stack_top = (uint32_t)esp_cpu_get_sp();
    mp_stack_set_top((void *)stack_top);
    mp_stack_set_limit(PHASE0_MPY_STACK_BYTES - 1024);

    mp_obj_t module = mp_obj_new_module(MP_QSTR___main__);
    mp_obj_dict_t *globals = mp_obj_module_get_globals(module);
    mp_obj_dict_t *locals = globals;
    mp_obj_dict_store(
        MP_OBJ_FROM_PTR(globals),
        MP_OBJ_NEW_QSTR(qstr_from_str("__tool_code")),
        mp_obj_new_str(code, strlen(code))
    );

    nlr_buf_t nlr;
    if (nlr_push(&nlr) == 0) {
        mp_lexer_t *lex = mp_lexer_new_from_str_len(
            MP_QSTR__lt_stdin_gt_,
            WRAPPER_SRC,
            strlen(WRAPPER_SRC),
            0
        );
        mp_obj_t ignored = mp_parse_compile_execute(lex, MP_PARSE_FILE_INPUT, globals, locals);
        (void)ignored;
        size_t stdout_len = 0;
        size_t stderr_len = 0;
        size_t result_len = 0;
        size_t error_len = 0;
        const char *stdout_text = phase0_opt_str(globals, "__tool_stdout", &stdout_len);
        const char *stderr_text = phase0_opt_str(globals, "__tool_stderr", &stderr_len);
        const char *result_text = phase0_opt_str(globals, "__tool_result", &result_len);
        const char *error_text = phase0_opt_str(globals, "__tool_error", &error_len);
        phase0_fill_payload_json(
            out_result,
            out_result->job_id,
            out_result->background,
            error_text == NULL || error_len == 0,
            error_text == NULL || error_len == 0 ? 0 : -1,
            stdout_text,
            stdout_len,
            stderr_text,
            stderr_len,
            result_text,
            result_len,
            error_text,
            error_len
        );
        ok = true;
        nlr_pop();
    } else {
        mp_obj_t exception = MP_OBJ_FROM_PTR(nlr.ret_val);
        vstr_t vstr;
        vstr_init(&vstr, 256);
        mp_print_t print = {&vstr, (mp_print_strn_t)vstr_add_strn};
        mp_obj_print_exception(&print, exception);
        phase0_fill_result(out_result, out_result->job_id, out_result->background, false, -1, vstr_null_terminated_str(&vstr));
        vstr_clear(&vstr);
    }

    gc_collect();
    return ok;
}

static void phase0_process_request(phase0_mpy_request_t *request) {
    phase0_mpy_result_t local_result;
    phase0_fill_result(
        &local_result,
        request->job_id,
        request->background,
        false,
        -2,
        "{\"error\":\"interpreter_not_ready\"}"
    );

    bool ok = phase0_exec_wrapper(request->code, &local_result);
    if (request->sync_result) {
        *(request->sync_result) = local_result;
    } else if (s_result_queue) {
        if (xQueueSend(s_result_queue, &local_result, 0) != pdTRUE) {
            ESP_LOGW(TAG, "background result queue full for job %lu", (unsigned long)request->job_id);
        }
    } else if (!ok) {
        ESP_LOGE(TAG, "background job %lu failed: %s", (unsigned long)request->job_id, local_result.payload);
    } else {
        ESP_LOGI(TAG, "background job %lu payload=%s", (unsigned long)request->job_id, local_result.payload);
    }
}

static void phase0_mpy_task(void *arg) {
    (void)arg;

#if MICROPY_PY_THREAD
    mp_thread_init(pxTaskGetStackStart(NULL), s_mpy_stack_bytes / sizeof(uintptr_t));
#endif

#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
    usb_serial_jtag_init();
#elif CONFIG_USB_OTG_SUPPORTED
    usb_init();
#endif
#if MICROPY_HW_ENABLE_UART_REPL
    uart_stdout_init();
#endif

    machine_init();

    void *gc_heap = heap_caps_malloc(
        PHASE0_MPY_HEAP_BYTES,
        MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT
    );
    if (!gc_heap) {
        gc_heap = heap_caps_malloc(PHASE0_MPY_HEAP_BYTES, MALLOC_CAP_8BIT);
    }
    if (!gc_heap) {
        ESP_LOGE(
            TAG,
            "gc heap alloc failed bytes=%u internal_free=%u spiram_free=%u",
            (unsigned)PHASE0_MPY_HEAP_BYTES,
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
        );
        s_init_result = ESP_ERR_NO_MEM;
        xSemaphoreGive(s_init_done);
        vTaskDelete(NULL);
        return;
    }

    uint32_t stack_top = (uint32_t)esp_cpu_get_sp();
    mp_stack_set_top((void *)stack_top);
    mp_stack_set_limit(s_mpy_stack_bytes - 1024);
    gc_init(gc_heap, (uint8_t *)gc_heap + PHASE0_MPY_HEAP_BYTES);
    mp_init();
    machine_pins_init();

    printf(
        "MKT:MICROPY:TASK_READY stack_bytes=%u stack_in_spiram=%s internal_free=%u spiram_free=%u\n",
        (unsigned)s_mpy_stack_bytes,
        s_mpy_stack_in_spiram ? "true" : "false",
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
    );
    ESP_LOGI(
        TAG,
        "init ok stack_bytes=%u heap_bytes=%u internal_free=%u spiram_free=%u",
        (unsigned)s_mpy_stack_bytes,
        (unsigned)PHASE0_MPY_HEAP_BYTES,
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
    );
    s_init_result = ESP_OK;
    xSemaphoreGive(s_init_done);

    for (;;) {
        phase0_mpy_request_t request = { 0 };
        if (xQueueReceive(s_request_queue, &request, portMAX_DELAY) != pdTRUE) {
            continue;
        }
        phase0_process_request(&request);
        if (request.sync_done) {
            xSemaphoreGive(request.sync_done);
        }
        free(request.code);
    }
}

esp_err_t phase0_mpy_init(void) {
    if (s_mpy_task) {
        return s_init_result;
    }

    ESP_LOGI(
        TAG,
        "init start internal_free=%u spiram_free=%u",
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
    );
    printf(
        "MKT:MICROPY:INIT_START internal_free=%u spiram_free=%u\n",
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
    );

    s_request_queue = xQueueCreate(PHASE0_MPY_QUEUE_DEPTH, sizeof(phase0_mpy_request_t));
    if (!s_request_queue) {
        ESP_LOGE(TAG, "request queue alloc failed");
        return ESP_ERR_NO_MEM;
    }

    s_result_queue = xQueueCreate(PHASE0_MPY_QUEUE_DEPTH, sizeof(phase0_mpy_result_t));
    if (!s_result_queue) {
        ESP_LOGE(TAG, "result queue alloc failed");
        return ESP_ERR_NO_MEM;
    }

    s_init_done = xSemaphoreCreateBinary();
    if (!s_init_done) {
        ESP_LOGE(TAG, "init semaphore alloc failed");
        return ESP_ERR_NO_MEM;
    }

    const size_t spiram_stack_candidates[] = { PHASE0_MPY_STACK_BYTES, 40 * 1024, 32 * 1024, 24 * 1024 };
    const size_t internal_stack_candidates[] = { 24 * 1024, 20 * 1024, 16 * 1024, 12 * 1024 };
    BaseType_t spawned = pdFAIL;
    for (size_t i = 0; i < sizeof(spiram_stack_candidates) / sizeof(spiram_stack_candidates[0]); ++i) {
        s_mpy_stack_bytes = spiram_stack_candidates[i];
        s_mpy_stack_in_spiram = true;
        printf(
            "MKT:MICROPY:SPAWN_TRY stack_bytes=%u stack_in_spiram=true internal_free=%u spiram_free=%u\n",
            (unsigned)s_mpy_stack_bytes,
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
        );
        spawned = xTaskCreatePinnedToCoreWithCaps(
            phase0_mpy_task,
            "phase0-mpy",
            s_mpy_stack_bytes,
            NULL,
            ESP_TASK_PRIO_MIN + 1,
            &s_mpy_task,
            1,
            MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT
        );
        if (spawned == pdPASS) {
            printf("MKT:MICROPY:SPAWN_OK stack_bytes=%u stack_in_spiram=true\n", (unsigned)s_mpy_stack_bytes);
            ESP_LOGI(TAG, "spawned MicroPython task with stack_bytes=%u in SPIRAM", (unsigned)s_mpy_stack_bytes);
            break;
        }
        printf(
            "MKT:MICROPY:SPAWN_FAIL stack_bytes=%u stack_in_spiram=true internal_free=%u spiram_free=%u\n",
            (unsigned)s_mpy_stack_bytes,
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
        );
        ESP_LOGW(
            TAG,
            "task spawn failed stack_bytes=%u internal_free=%u spiram_free=%u",
            (unsigned)s_mpy_stack_bytes,
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
            (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
        );
        s_mpy_task = NULL;
    }
    if (spawned != pdPASS) {
        for (size_t i = 0; i < sizeof(internal_stack_candidates) / sizeof(internal_stack_candidates[0]); ++i) {
            s_mpy_stack_bytes = internal_stack_candidates[i];
            s_mpy_stack_in_spiram = false;
            printf(
                "MKT:MICROPY:SPAWN_TRY stack_bytes=%u stack_in_spiram=false internal_free=%u spiram_free=%u\n",
                (unsigned)s_mpy_stack_bytes,
                (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
                (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
            );
            spawned = xTaskCreatePinnedToCore(
                phase0_mpy_task,
                "phase0-mpy",
                s_mpy_stack_bytes,
                NULL,
                ESP_TASK_PRIO_MIN + 1,
                &s_mpy_task,
                1
            );
            if (spawned == pdPASS) {
                printf("MKT:MICROPY:SPAWN_OK stack_bytes=%u stack_in_spiram=false\n", (unsigned)s_mpy_stack_bytes);
                break;
            }
            printf(
                "MKT:MICROPY:SPAWN_FAIL stack_bytes=%u stack_in_spiram=false internal_free=%u spiram_free=%u\n",
                (unsigned)s_mpy_stack_bytes,
                (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
                (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM)
            );
            s_mpy_task = NULL;
        }
    }
    if (spawned != pdPASS) {
        printf("MKT:MICROPY:INIT_FAIL reason=task_spawn\n");
        return ESP_FAIL;
    }

    if (xSemaphoreTake(s_init_done, pdMS_TO_TICKS(PHASE0_MPY_TIMEOUT_MS)) != pdTRUE) {
        ESP_LOGE(TAG, "init timeout waiting for task");
        printf("MKT:MICROPY:INIT_FAIL reason=timeout_waiting_for_task\n");
        return ESP_ERR_TIMEOUT;
    }

    printf(
        "MKT:MICROPY:INIT_RESULT esp_err=%d stack_bytes=%u stack_in_spiram=%s\n",
        (int)s_init_result,
        (unsigned)s_mpy_stack_bytes,
        s_mpy_stack_in_spiram ? "true" : "false"
    );
    return s_init_result;
}

esp_err_t phase0_mpy_exec_sync(
    const char *code,
    uint32_t timeout_ms,
    phase0_mpy_result_t *out_result
) {
    if (!code || !out_result) {
        return ESP_ERR_INVALID_ARG;
    }
    if (!s_mpy_task) {
        esp_err_t init_err = phase0_mpy_init();
        if (init_err != ESP_OK) {
            return init_err;
        }
    }

    SemaphoreHandle_t sync_done = xSemaphoreCreateBinary();
    if (!sync_done) {
        return ESP_ERR_NO_MEM;
    }

    char *owned_code = strdup(code);
    if (!owned_code) {
        vSemaphoreDelete(sync_done);
        return ESP_ERR_NO_MEM;
    }

    memset(out_result, 0, sizeof(*out_result));
    out_result->job_id = s_next_job_id++;

    phase0_mpy_request_t request = {
        .job_id = out_result->job_id,
        .background = false,
        .code = owned_code,
        .sync_result = out_result,
        .sync_done = sync_done,
    };

    if (xQueueSend(s_request_queue, &request, pdMS_TO_TICKS(500)) != pdTRUE) {
        free(owned_code);
        vSemaphoreDelete(sync_done);
        return ESP_ERR_TIMEOUT;
    }

    TickType_t wait_ticks = pdMS_TO_TICKS(timeout_ms ? timeout_ms : PHASE0_MPY_TIMEOUT_MS);
    if (xSemaphoreTake(sync_done, wait_ticks) != pdTRUE) {
        phase0_fill_result(
            out_result,
            out_result->job_id,
            false,
            false,
            -3,
            "{\"error\":\"timeout\"}"
        );
        vSemaphoreDelete(sync_done);
        return ESP_ERR_TIMEOUT;
    }

    vSemaphoreDelete(sync_done);
    return ESP_OK;
}

esp_err_t phase0_mpy_submit_async(const char *code, uint32_t *out_job_id) {
    if (!code || !out_job_id) {
        return ESP_ERR_INVALID_ARG;
    }
    if (!s_mpy_task) {
        esp_err_t init_err = phase0_mpy_init();
        if (init_err != ESP_OK) {
            return init_err;
        }
    }

    char *owned_code = strdup(code);
    if (!owned_code) {
        return ESP_ERR_NO_MEM;
    }

    uint32_t job_id = s_next_job_id++;
    phase0_mpy_request_t request = {
        .job_id = job_id,
        .background = true,
        .code = owned_code,
        .sync_result = NULL,
        .sync_done = NULL,
    };

    if (xQueueSend(s_request_queue, &request, pdMS_TO_TICKS(500)) != pdTRUE) {
        free(owned_code);
        return ESP_ERR_TIMEOUT;
    }

    *out_job_id = job_id;
    return ESP_OK;
}

esp_err_t phase0_mpy_poll_result(phase0_mpy_result_t *out_result) {
    if (!out_result) {
        return ESP_ERR_INVALID_ARG;
    }
    if (!s_result_queue) {
        return ESP_ERR_INVALID_STATE;
    }
    if (xQueueReceive(s_result_queue, out_result, 0) != pdTRUE) {
        return ESP_ERR_NOT_FOUND;
    }
    return ESP_OK;
}

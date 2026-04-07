#include "ws147_vendor_smoke.h"

#include "driver/gpio.h"
#include "driver/ledc.h"
#include "driver/spi_master.h"
#include "esp_check.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_vendor.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "Vernon_ST7789T.h"

#define LCD_HOST SPI3_HOST
#define LCD_PIXEL_CLOCK_HZ (12 * 1000 * 1000)
#define PIN_SCLK 40
#define PIN_MOSI 45
#define PIN_MISO -1
#define PIN_DC 41
#define PIN_RST 39
#define PIN_CS 42
#define PIN_BL 48
#define LCD_H_RES 172
#define LCD_V_RES 320
#define LCD_CMD_BITS 8
#define LCD_PARAM_BITS 8

#define OFFSET_X 34
#define OFFSET_Y 0

#define LEDC_MODE LEDC_LOW_SPEED_MODE
#define LEDC_TIMER LEDC_TIMER_0
#define LEDC_CHANNEL LEDC_CHANNEL_0
#define LEDC_RESOLUTION LEDC_TIMER_13_BIT
#define LEDC_MAX_DUTY ((1 << LEDC_RESOLUTION) - 1)

static const char *TAG = "ws147-vendor";

static esp_err_t backlight_init(void) {
    gpio_config_t bk_gpio_config = {
        .mode = GPIO_MODE_OUTPUT,
        .pin_bit_mask = 1ULL << PIN_BL,
    };
    ESP_RETURN_ON_ERROR(gpio_config(&bk_gpio_config), TAG, "bk gpio failed");

    ledc_timer_config_t ledc_timer = {
        .duty_resolution = LEDC_RESOLUTION,
        .freq_hz = 5000,
        .speed_mode = LEDC_MODE,
        .timer_num = LEDC_TIMER,
        .clk_cfg = LEDC_AUTO_CLK,
    };
    ESP_RETURN_ON_ERROR(ledc_timer_config(&ledc_timer), TAG, "ledc timer failed");

    ledc_channel_config_t ledc_channel = {
        .channel = LEDC_CHANNEL,
        .duty = 0,
        .gpio_num = PIN_BL,
        .speed_mode = LEDC_MODE,
        .timer_sel = LEDC_TIMER,
    };
    ESP_RETURN_ON_ERROR(ledc_channel_config(&ledc_channel), TAG, "ledc channel failed");
    ESP_RETURN_ON_ERROR(ledc_fade_func_install(0), TAG, "ledc fade install failed");
    return ESP_OK;
}

static esp_err_t backlight_set(uint8_t light) {
    if (light > 100) {
        light = 100;
    }
    uint32_t duty = LEDC_MAX_DUTY - (81 * (100 - light));
    if (light == 0) {
        duty = 0;
    }
    ESP_RETURN_ON_ERROR(ledc_set_duty(LEDC_MODE, LEDC_CHANNEL, duty), TAG, "set duty failed");
    ESP_RETURN_ON_ERROR(ledc_update_duty(LEDC_MODE, LEDC_CHANNEL), TAG, "update duty failed");
    return ESP_OK;
}

static esp_err_t fill_color(esp_lcd_panel_handle_t panel, uint16_t color) {
    static uint16_t line[LCD_H_RES];
    for (size_t i = 0; i < LCD_H_RES; ++i) {
        line[i] = color;
    }
    for (int y = 0; y < LCD_V_RES; ++y) {
        ESP_RETURN_ON_ERROR(
            esp_lcd_panel_draw_bitmap(panel, 0 + OFFSET_X, y + OFFSET_Y, LCD_H_RES + OFFSET_X, y + 1 + OFFSET_Y, line),
            TAG,
            "draw bitmap failed"
        );
    }
    return ESP_OK;
}

esp_err_t ws147_vendor_smoke_run(void) {
    spi_bus_config_t buscfg = {
        .sclk_io_num = PIN_SCLK,
        .mosi_io_num = PIN_MOSI,
        .miso_io_num = PIN_MISO,
        .quadwp_io_num = -1,
        .quadhd_io_num = -1,
        .max_transfer_sz = LCD_H_RES * LCD_V_RES * sizeof(uint16_t),
    };
    ESP_RETURN_ON_ERROR(spi_bus_initialize(LCD_HOST, &buscfg, SPI_DMA_CH_AUTO), TAG, "spi init failed");

    esp_lcd_panel_io_handle_t io_handle = NULL;
    esp_lcd_panel_io_spi_config_t io_config = {
        .dc_gpio_num = PIN_DC,
        .cs_gpio_num = PIN_CS,
        .pclk_hz = LCD_PIXEL_CLOCK_HZ,
        .lcd_cmd_bits = LCD_CMD_BITS,
        .lcd_param_bits = LCD_PARAM_BITS,
        .spi_mode = 0,
        .trans_queue_depth = 10,
    };
    ESP_RETURN_ON_ERROR(
        esp_lcd_new_panel_io_spi((esp_lcd_spi_bus_handle_t)LCD_HOST, &io_config, &io_handle),
        TAG,
        "panel io init failed"
    );

    esp_lcd_panel_dev_st7789t_config_t panel_config = {
        .reset_gpio_num = PIN_RST,
        .rgb_endian = LCD_RGB_ENDIAN_BGR,
        .bits_per_pixel = 16,
    };

    esp_lcd_panel_handle_t panel_handle = NULL;
    ESP_RETURN_ON_ERROR(
        esp_lcd_new_panel_st7789t(io_handle, &panel_config, &panel_handle),
        TAG,
        "new panel failed"
    );

    ESP_RETURN_ON_ERROR(esp_lcd_panel_reset(panel_handle), TAG, "panel reset failed");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_init(panel_handle), TAG, "panel init failed");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_mirror(panel_handle, true, false), TAG, "panel mirror failed");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_disp_on_off(panel_handle, true), TAG, "disp on failed");

    ESP_RETURN_ON_ERROR(backlight_init(), TAG, "backlight init failed");
    ESP_RETURN_ON_ERROR(backlight_set(75), TAG, "backlight set failed");

    ESP_RETURN_ON_ERROR(fill_color(panel_handle, 0xF800), TAG, "fill red failed");
    vTaskDelay(pdMS_TO_TICKS(1000));
    ESP_RETURN_ON_ERROR(fill_color(panel_handle, 0x07E0), TAG, "fill green failed");
    vTaskDelay(pdMS_TO_TICKS(1000));
    ESP_RETURN_ON_ERROR(fill_color(panel_handle, 0x001F), TAG, "fill blue failed");
    vTaskDelay(pdMS_TO_TICKS(1000));
    ESP_RETURN_ON_ERROR(fill_color(panel_handle, 0xFFFF), TAG, "fill white failed");
    vTaskDelay(pdMS_TO_TICKS(1000));
    ESP_RETURN_ON_ERROR(fill_color(panel_handle, 0x07E0), TAG, "fill final green failed");
    return ESP_OK;
}

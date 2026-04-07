idf_component_register(
    SRCS "ws147_vendor_smoke.c" "Vernon_ST7789T/Vernon_ST7789T.c"
    INCLUDE_DIRS "include" "Vernon_ST7789T"
    REQUIRES esp_lcd driver
)

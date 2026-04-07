cmake_minimum_required(VERSION 3.20)

set(IDF_TARGET $ENV{IDF_TARGET} CACHE STRING "IDF Build Target" FORCE)

set(EXTRA_COMPONENT_DIRS $ENV{EXTRA_COMPONENT_DIRS})
set(COMPONENTS $ENV{ESP_IDF_COMPONENTS})
set(SDKCONFIG $ENV{SDKCONFIG})
set(SDKCONFIG_DEFAULTS $ENV{SDKCONFIG_DEFAULTS})
set(ENV{CC} /usr/bin/clang)
set(ENV{CXX} /usr/bin/clang++)
set(ENV{CC} /usr/bin/clang)
set(ENV{CXX} /usr/bin/clang++)
set(ENV{MICROPY_MPYCROSS} /usr/bin/true)
set(ENV{SDKROOT} /Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk)
if(DEFINED ENV{PROJECT_DIR})
    get_filename_component(PHASE0_FIRMWARE_PROJECT_DIR "$ENV{PROJECT_DIR}" ABSOLUTE)
else()
    get_filename_component(PHASE0_FIRMWARE_PROJECT_DIR "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
endif()
set(PHASE0_PARTITION_CSV_SOURCE "${PHASE0_FIRMWARE_PROJECT_DIR}/partitions-phase0.csv")
set(PHASE0_PARTITION_CSV_BUILD_DIR "${CMAKE_BINARY_DIR}")
set(PHASE0_PARTITION_CSV_BUILD "${PHASE0_PARTITION_CSV_BUILD_DIR}/partitions-phase0.csv")
get_filename_component(PHASE0_PARTITION_CSV_PARENT_DIR "${CMAKE_BINARY_DIR}/.." ABSOLUTE)
set(PHASE0_PARTITION_CSV_PARENT_BUILD "${PHASE0_PARTITION_CSV_PARENT_DIR}/partitions-phase0.csv")

if(EXISTS "${PHASE0_PARTITION_CSV_SOURCE}")
    file(MAKE_DIRECTORY "${PHASE0_PARTITION_CSV_BUILD_DIR}")
    configure_file(
        "${PHASE0_PARTITION_CSV_SOURCE}"
        "${PHASE0_PARTITION_CSV_BUILD}"
        COPYONLY
    )
    file(MAKE_DIRECTORY "${PHASE0_PARTITION_CSV_PARENT_DIR}")
    configure_file(
        "${PHASE0_PARTITION_CSV_SOURCE}"
        "${PHASE0_PARTITION_CSV_PARENT_BUILD}"
        COPYONLY
    )
endif()

if(NOT MICROPY_HELPER_DIR)
    get_filename_component(
        MICROPY_HELPER_DIR
        "${CMAKE_BINARY_DIR}/../managed_components/mr9you__micropython-helper"
        ABSOLUTE
    )
endif()

set(MICROPY_BOARD ESP32_GENERIC_S3 CACHE STRING "MicroPython board" FORCE)
set(MICROPY_BOARD_VARIANT SPIRAM_OCT CACHE STRING "MicroPython board variant" FORCE)
set(MICROPY_FROZEN_MANIFEST ${CMAKE_CURRENT_LIST_DIR}/manifest_empty.py CACHE STRING "Disable frozen manifest for phase0 MicroPython tool spike" FORCE)

if(EXISTS ${MICROPY_HELPER_DIR})
    set(MICROPY_BOARD_HEADER ${MICROPY_HELPER_DIR}/micropython/ports/esp32/boards/${MICROPY_BOARD}/mpconfigboard.h)
    set(MICROPY_BOARD_SDKCONFIG ${MICROPY_HELPER_DIR}/micropython/ports/esp32/boards/${MICROPY_BOARD}/sdkconfig.board)
    set(MICROPY_BOARD_BASE_SDKCONFIG ${MICROPY_HELPER_DIR}/micropython/ports/esp32/boards/sdkconfig.base)
    set(MICROPY_NETWORK_COMMON ${MICROPY_HELPER_DIR}/micropython/ports/esp32/network_common.c)
    set(MICROPY_ESP32_COMMON_CMAKE ${MICROPY_HELPER_DIR}/esp32_common.cmake)
    set(MICROPY_PORT_CONFIG ${MICROPY_HELPER_DIR}/micropython/ports/esp32/mpconfigport.h)
    set(MICROPY_MODSOCKET_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp32/modsocket.c)
    set(MICROPY_MODESP_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp8266/modesp.c)
    if(EXISTS ${MICROPY_BOARD_HEADER})
        file(READ ${MICROPY_BOARD_HEADER} MICROPY_BOARD_HEADER_CONTENTS)
        string(REPLACE
            "#define MICROPY_HW_ENABLE_UART_REPL         (1)"
            "#define MICROPY_HW_ENABLE_UART_REPL         (0)"
            MICROPY_BOARD_HEADER_CONTENTS
            "${MICROPY_BOARD_HEADER_CONTENTS}"
        )
        file(WRITE ${MICROPY_BOARD_HEADER} "${MICROPY_BOARD_HEADER_CONTENTS}")
        configure_file(
            ${MICROPY_BOARD_HEADER}
            ${MICROPY_HELPER_DIR}/micropython/ports/esp32/mpconfigboard.h
            COPYONLY
        )
    endif()
    if(EXISTS ${MICROPY_BOARD_BASE_SDKCONFIG})
        file(READ ${MICROPY_BOARD_BASE_SDKCONFIG} MICROPY_BOARD_BASE_SDKCONFIG_CONTENTS)
        foreach(line
            "CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y"
            "CONFIG_PARTITION_TABLE_CUSTOM=y"
            "CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"partitions-4MiB.csv\""
        )
            string(REPLACE "${line}" "" MICROPY_BOARD_BASE_SDKCONFIG_CONTENTS "${MICROPY_BOARD_BASE_SDKCONFIG_CONTENTS}")
        endforeach()
        file(WRITE ${MICROPY_BOARD_BASE_SDKCONFIG} "${MICROPY_BOARD_BASE_SDKCONFIG_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_BOARD_SDKCONFIG})
        file(READ ${MICROPY_BOARD_SDKCONFIG} MICROPY_BOARD_SDKCONFIG_CONTENTS)
        foreach(line
            "CONFIG_ESPTOOLPY_FLASHMODE_QIO=y"
            "CONFIG_ESPTOOLPY_FLASHFREQ_80M=y"
            "CONFIG_ESPTOOLPY_AFTER_NORESET=y"
            "CONFIG_ESPTOOLPY_FLASHSIZE_4MB="
            "CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y"
            "CONFIG_ESPTOOLPY_FLASHSIZE_16MB="
            "CONFIG_PARTITION_TABLE_CUSTOM=y"
            "CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"partitions-8MiB.csv\""
        )
            string(REPLACE "${line}" "" MICROPY_BOARD_SDKCONFIG_CONTENTS "${MICROPY_BOARD_SDKCONFIG_CONTENTS}")
        endforeach()
        file(WRITE ${MICROPY_BOARD_SDKCONFIG} "${MICROPY_BOARD_SDKCONFIG_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_NETWORK_COMMON})
        file(READ ${MICROPY_NETWORK_COMMON} MICROPY_NETWORK_COMMON_CONTENTS)
        string(REPLACE
            "#if ESP_IDF_VERSION > ESP_IDF_VERSION_VAL(5, 1, 1)\n_Static_assert(WIFI_AUTH_MAX == 11, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#else\n_Static_assert(WIFI_AUTH_MAX == 10, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#endif"
            "#if ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(5, 3, 0)\n_Static_assert(WIFI_AUTH_MAX == 14, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#elif ESP_IDF_VERSION > ESP_IDF_VERSION_VAL(5, 1, 1)\n_Static_assert(WIFI_AUTH_MAX == 13, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#else\n_Static_assert(WIFI_AUTH_MAX == 10, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#endif"
            MICROPY_NETWORK_COMMON_CONTENTS
            "${MICROPY_NETWORK_COMMON_CONTENTS}"
        )
        string(REPLACE
            "#if ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(5, 3, 0)\n_Static_assert(WIFI_AUTH_MAX == 14, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#elif ESP_IDF_VERSION > ESP_IDF_VERSION_VAL(5, 1, 1)\n_Static_assert(WIFI_AUTH_MAX == 14, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#else\n_Static_assert(WIFI_AUTH_MAX == 10, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#endif"
            "#if ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(5, 3, 0)\n_Static_assert(WIFI_AUTH_MAX == 14, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#elif ESP_IDF_VERSION > ESP_IDF_VERSION_VAL(5, 1, 1)\n_Static_assert(WIFI_AUTH_MAX == 13, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#else\n_Static_assert(WIFI_AUTH_MAX == 10, \"Synchronize WIFI_AUTH_XXX constants with the ESP-IDF. Look at esp-idf/components/esp_wifi/include/esp_wifi_types.h\");\n#endif"
            MICROPY_NETWORK_COMMON_CONTENTS
            "${MICROPY_NETWORK_COMMON_CONTENTS}"
        )
        file(WRITE ${MICROPY_NETWORK_COMMON} "${MICROPY_NETWORK_COMMON_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_ESP32_COMMON_CMAKE})
        file(READ ${MICROPY_ESP32_COMMON_CMAKE} MICROPY_ESP32_COMMON_CONTENTS)
        foreach(rewrite
            "    set(MICROPY_PY_BTREE ON)|    set(MICROPY_PY_BTREE OFF)"
            "    ${MICROPY_DIR}/shared/netutils/netutils.c|"
            "    ppp_set_auth.c|"
            "    network_common.c|"
            "    network_lan.c|"
            "    network_ppp.c|"
            "    network_wlan.c|"
            "    mpnimbleport.c|"
            "    modsocket.c|"
            "    modesp.c|"
            "    machine_sdcard.c|"
            "    machine_bitstream.c|"
            "    modespnow.c|"
            "    esp32_ulp.c|"
            "    esp32_rmt.c|"
            "    mpthreadport.c|"
            "    usb.c|"
            "    bt|"
            "    esp_eth|"
            "    esp_event|"
            "    esp_netif|"
            "    esp_ringbuf|"
            "    esp_wifi|"
            "    lwip|"
            "    sdmmc|"
            "    ulp|"
        )
            string(REPLACE "|" ";" rewrite_parts "${rewrite}")
            list(GET rewrite_parts 0 rewrite_from)
            list(GET rewrite_parts 1 rewrite_to)
            string(REPLACE "${rewrite_from}\n" "${rewrite_to}" MICROPY_ESP32_COMMON_CONTENTS "${MICROPY_ESP32_COMMON_CONTENTS}")
        endforeach()
        string(REGEX REPLACE
            "set\\(MICROPY_PY_BTREE ON\\)"
            "set(MICROPY_PY_BTREE OFF)"
            MICROPY_ESP32_COMMON_CONTENTS
            "${MICROPY_ESP32_COMMON_CONTENTS}"
        )
        foreach(line
            "\\$\\{MICROPY_DIR\\}/shared/netutils/netutils\\.c"
            "ppp_set_auth\\.c"
            "network_common\\.c"
            "network_lan\\.c"
            "network_ppp\\.c"
            "network_wlan\\.c"
            "mpnimbleport\\.c"
            "modsocket\\.c"
            "modesp\\.c"
            "machine_sdcard\\.c"
            "machine_bitstream\\.c"
            "modespnow\\.c"
            "esp32_ulp\\.c"
            "esp32_rmt\\.c"
            "mpthreadport\\.c"
            "usb\\.c"
        )
            string(REGEX REPLACE
                "[ \t]*${line}\n"
                ""
                MICROPY_ESP32_COMMON_CONTENTS
                "${MICROPY_ESP32_COMMON_CONTENTS}"
            )
        endforeach()
        foreach(line
            "bt"
            "esp_eth"
            "esp_event"
            "esp_netif"
            "esp_ringbuf"
            "esp_wifi"
            "lwip"
            "sdmmc"
            "ulp"
        )
            string(REGEX REPLACE
                "[ \t]*${line}\n"
                ""
                MICROPY_ESP32_COMMON_CONTENTS
                "${MICROPY_ESP32_COMMON_CONTENTS}"
            )
        endforeach()
        string(REGEX REPLACE
            "[ \t]*target_link_libraries\\(\\$\\{MICROPY_TARGET\\}[ \t]+micropy_extmod_btree\\)\n"
            ""
            MICROPY_ESP32_COMMON_CONTENTS
            "${MICROPY_ESP32_COMMON_CONTENTS}"
        )
        string(REPLACE
            "target_link_libraries(${MICROPY_TARGET} micropy_extmod_btree)"
            ""
            MICROPY_ESP32_COMMON_CONTENTS
            "${MICROPY_ESP32_COMMON_CONTENTS}"
        )
        file(WRITE ${MICROPY_ESP32_COMMON_CMAKE} "${MICROPY_ESP32_COMMON_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_PORT_CONFIG})
        file(READ ${MICROPY_PORT_CONFIG} MICROPY_PORT_CONFIG_CONTENTS)
        string(REPLACE
            "#include \"esp_wifi_types.h\""
            "/* phase0 micropython spike: wifi types disabled */"
            MICROPY_PORT_CONFIG_CONTENTS
            "${MICROPY_PORT_CONFIG_CONTENTS}"
        )
        foreach(rewrite
            "#define MICROPY_PY_THREAD                   (1)|#define MICROPY_PY_THREAD                   (0)"
            "#define MICROPY_PY_THREAD_GIL               (1)|#define MICROPY_PY_THREAD_GIL               (0)"
            "#define MICROPY_PY_ESPNOW                   (1)|#define MICROPY_PY_ESPNOW                   (0)"
            "#define MICROPY_PY_BLUETOOTH                (1)|#define MICROPY_PY_BLUETOOTH                (0)"
            "#define MICROPY_PY_NETWORK (1)|#define MICROPY_PY_NETWORK (0)"
            "#define MICROPY_PY_NETWORK_WLAN             (1)|#define MICROPY_PY_NETWORK_WLAN             (0)"
            "#define MICROPY_HW_ENABLE_SDCARD            (1)|#define MICROPY_HW_ENABLE_SDCARD            (0)"
            "#define MICROPY_PY_SSL                      (1)|#define MICROPY_PY_SSL                      (0)"
            "#define MICROPY_PY_WEBSOCKET                (1)|#define MICROPY_PY_WEBSOCKET                (0)"
            "#define MICROPY_PY_WEBREPL                  (1)|#define MICROPY_PY_WEBREPL                  (0)"
        )
            string(REPLACE "|" ";" rewrite_parts "${rewrite}")
            list(GET rewrite_parts 0 rewrite_from)
            list(GET rewrite_parts 1 rewrite_to)
            string(REPLACE "${rewrite_from}" "${rewrite_to}" MICROPY_PORT_CONFIG_CONTENTS "${MICROPY_PORT_CONFIG_CONTENTS}")
        endforeach()
        foreach(rewrite
            "MICROPY_PY_THREAD"
            "MICROPY_PY_THREAD_GIL"
            "MICROPY_PY_ESPNOW"
            "MICROPY_PY_BLUETOOTH"
            "MICROPY_PY_NETWORK"
            "MICROPY_PY_NETWORK_WLAN"
            "MICROPY_HW_ENABLE_SDCARD"
            "MICROPY_PY_SSL"
            "MICROPY_PY_WEBSOCKET"
            "MICROPY_PY_WEBREPL"
        )
            string(REGEX REPLACE
                "(#define[ \t]+${rewrite}[ \t]+)\\(1\\)"
                "\\1(0)"
                MICROPY_PORT_CONFIG_CONTENTS
                "${MICROPY_PORT_CONFIG_CONTENTS}"
            )
        endforeach()
        file(WRITE ${MICROPY_PORT_CONFIG} "${MICROPY_PORT_CONFIG_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_ESP32_COMMON_CMAKE})
        file(READ ${MICROPY_ESP32_COMMON_CMAKE} MICROPY_ESP32_COMMON_CONTENTS)
        foreach(line
            "CONFIG_SPIRAM_MODE_QUAD=y"
            "CONFIG_SPIRAM_TYPE_AUTO=y"
            "CONFIG_SPIRAM_CLK_IO=30"
            "CONFIG_SPIRAM_CS_IO=26"
            "CONFIG_SPIRAM_SPEED_80M=y"
            "CONFIG_SPIRAM=y"
            "CONFIG_SPIRAM_BOOT_INIT=y"
            "CONFIG_SPIRAM_IGNORE_NOTFOUND=y"
            "CONFIG_SPIRAM_USE_MALLOC=y"
            "CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=8192"
        )
            string(REPLACE "${line}" "" MICROPY_ESP32_COMMON_CONTENTS "${MICROPY_ESP32_COMMON_CONTENTS}")
        endforeach()
        file(WRITE ${MICROPY_ESP32_COMMON_CMAKE} "${MICROPY_ESP32_COMMON_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_MODSOCKET_SOURCE})
        file(READ ${MICROPY_MODSOCKET_SOURCE} MICROPY_MODSOCKET_CONTENTS)
        string(REPLACE
            "MP_REGISTER_EXTENSIBLE_MODULE(MP_QSTR_socket, mp_module_socket);"
            "/* phase0 micropython spike: socket registration disabled */"
            MICROPY_MODSOCKET_CONTENTS
            "${MICROPY_MODSOCKET_CONTENTS}"
        )
        file(WRITE ${MICROPY_MODSOCKET_SOURCE} "${MICROPY_MODSOCKET_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_MODESP_SOURCE})
        file(READ ${MICROPY_MODESP_SOURCE} MICROPY_MODESP_CONTENTS)
        string(REPLACE
            "MP_REGISTER_MODULE(MP_QSTR_esp, esp_module);"
            "/* phase0 micropython spike: esp registration disabled */"
            MICROPY_MODESP_CONTENTS
            "${MICROPY_MODESP_CONTENTS}"
        )
        file(WRITE ${MICROPY_MODESP_SOURCE} "${MICROPY_MODESP_CONTENTS}")
    endif()
    include(${MICROPY_HELPER_DIR}/mpy.cmake)

    # The helper generates sdkconfig.combined from its board defaults after our
    # top-level sdkconfig.defaults has already been chosen. Patch the generated
    # combined config directly so the real board geometry wins deterministically.
    foreach(micropy_generated_sdkconfig
        ${CMAKE_BINARY_DIR}/sdkconfig.combined.in
        ${CMAKE_BINARY_DIR}/sdkconfig.combined
    )
        if(EXISTS ${micropy_generated_sdkconfig})
            file(READ ${micropy_generated_sdkconfig} MICROPY_GENERATED_SDKCONFIG_CONTENTS)
            foreach(line
                "CONFIG_ESPTOOLPY_FLASHMODE_QIO=y"
                "CONFIG_ESPTOOLPY_FLASHFREQ_80M=y"
                "CONFIG_ESPTOOLPY_AFTER_NORESET=y"
                "CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y"
                "CONFIG_ESPTOOLPY_FLASHSIZE_4MB="
                "CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y"
                "CONFIG_ESPTOOLPY_FLASHSIZE_16MB="
                "CONFIG_PARTITION_TABLE_CUSTOM=y"
                "CONFIG_SPIRAM_MODE_QUAD=y"
                "CONFIG_SPIRAM_TYPE_AUTO=y"
                "CONFIG_SPIRAM_CLK_IO=30"
                "CONFIG_SPIRAM_CS_IO=26"
                "CONFIG_SPIRAM_SPEED_80M=y"
                "CONFIG_SPIRAM=y"
                "CONFIG_SPIRAM_BOOT_INIT=y"
                "CONFIG_SPIRAM_IGNORE_NOTFOUND=y"
                "CONFIG_SPIRAM_USE_MALLOC=y"
                "CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=8192"
                "CONFIG_ESP_CONSOLE_UART_DEFAULT=y"
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y"
                "CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=y"
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED=y"
                "CONFIG_ESP_CONSOLE_UART=y"
                "CONFIG_ESP_CONSOLE_UART_NUM=0"
                "CONFIG_ESP_CONSOLE_UART_BAUDRATE=115200"
                "CONFIG_ESP_CONSOLE_SECONDARY_NONE=y"
            )
                string(REPLACE "${line}" "" MICROPY_GENERATED_SDKCONFIG_CONTENTS "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}")
            endforeach()
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_UART_DEFAULT=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_UART=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_UART_NUM=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_UART_BAUDRATE=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_ESP_CONSOLE_SECONDARY_NONE=[^\n]*\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"[^\"]*partitions-4MiB\\.csv\"\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(REGEX REPLACE
                "CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"[^\"]*partitions-8MiB\\.csv\"\n?"
                ""
                MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}"
            )
            string(APPEND MICROPY_GENERATED_SDKCONFIG_CONTENTS
                "\n# phase0 micropython spike: force real board geometry\n"
                "CONFIG_SPIRAM=y\n"
                "CONFIG_ESP32S3_SPIRAM_SUPPORT=y\n"
                "CONFIG_SPIRAM_BOOT_INIT=y\n"
                "CONFIG_SPIRAM_IGNORE_NOTFOUND=n\n"
                "CONFIG_SPIRAM_MODE_QUAD=\n"
                "CONFIG_SPIRAM_MODE_OCT=y\n"
                "CONFIG_SPIRAM_TYPE_AUTO=y\n"
                "CONFIG_SPIRAM_SPEED_80M=y\n"
                "CONFIG_SPIRAM_USE_MALLOC=y\n"
                "CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=0\n"
                "CONFIG_ESPTOOLPY_FLASHSIZE_8MB=\n"
                "CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y\n"
                "CONFIG_ESPTOOLPY_FLASHSIZE=\"16MB\"\n"
                "CONFIG_PARTITION_TABLE_SINGLE_APP_LARGE=\n"
                "CONFIG_PARTITION_TABLE_CUSTOM=y\n"
                "CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"partitions-phase0.csv\"\n"
                "CONFIG_ESP_CONSOLE_UART_DEFAULT=\n"
                "CONFIG_ESP_CONSOLE_UART=\n"
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y\n"
                "CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED=y\n"
                "CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=\n"
                "CONFIG_ESP_CONSOLE_SECONDARY_NONE=y\n"
            )
            file(WRITE ${micropy_generated_sdkconfig} "${MICROPY_GENERATED_SDKCONFIG_CONTENTS}")
        endif()
    endforeach()
endif()

function(phase0_force_patch_generated_micropy_helper helper_dir)
    if(NOT EXISTS ${helper_dir})
        return()
    endif()

    set(board_dir ${helper_dir}/micropython/ports/esp32/boards/${MICROPY_BOARD})
    set(port_config ${helper_dir}/micropython/ports/esp32/mpconfigport.h)
    set(common_cmake_candidates
        ${helper_dir}/micropython/ports/esp32/esp32_common.cmake
        ${helper_dir}/esp32_common.cmake
    )

    if(EXISTS ${port_config})
        file(READ ${port_config} port_config_contents)
        string(REPLACE
            "#include \"mpconfigboard.h\""
            "#include \"boards/${MICROPY_BOARD}/mpconfigboard.h\""
            port_config_contents
            "${port_config_contents}"
        )
        string(REPLACE
            "#include \"esp_wifi_types.h\""
            "/* phase0 micropython spike: wifi types disabled */"
            port_config_contents
            "${port_config_contents}"
        )
        foreach(symbol
            MICROPY_PY_THREAD
            MICROPY_PY_THREAD_GIL
            MICROPY_PY_ESPNOW
            MICROPY_PY_BLUETOOTH
            MICROPY_PY_NETWORK
            MICROPY_PY_NETWORK_WLAN
            MICROPY_HW_ENABLE_SDCARD
            MICROPY_PY_SSL
            MICROPY_PY_WEBSOCKET
            MICROPY_PY_WEBREPL
        )
            string(REGEX REPLACE
                "(#define[ \t]+${symbol}[ \t]+)\\(1\\)"
                "\\1(0)"
                port_config_contents
                "${port_config_contents}"
            )
        endforeach()
        file(WRITE ${port_config} "${port_config_contents}")
    endif()

    foreach(common_cmake ${common_cmake_candidates})
        if(EXISTS ${common_cmake})
            file(READ ${common_cmake} common_cmake_contents)
            string(REPLACE
                "set(MICROPY_PY_BTREE ON)"
                "set(MICROPY_PY_BTREE OFF)"
                common_cmake_contents
                "${common_cmake_contents}"
            )
            foreach(line
                "\\$\\{MICROPY_DIR\\}/shared/netutils/netutils\\.c"
                "ppp_set_auth\\.c"
                "network_common\\.c"
                "network_lan\\.c"
                "network_ppp\\.c"
                "network_wlan\\.c"
                "mpnimbleport\\.c"
                "modsocket\\.c"
                "modesp\\.c"
                "machine_sdcard\\.c"
                "machine_bitstream\\.c"
                "modespnow\\.c"
                "esp32_ulp\\.c"
                "esp32_rmt\\.c"
                "mpthreadport\\.c"
                "usb\\.c"
            )
                string(REGEX REPLACE
                    "[ \t]*${line}\n"
                    ""
                    common_cmake_contents
                    "${common_cmake_contents}"
                )
            endforeach()
            foreach(line
                "bt"
                "esp_eth"
                "esp_event"
                "esp_netif"
                "esp_ringbuf"
                "esp_wifi"
                "lwip"
                "sdmmc"
                "ulp"
            )
                string(REGEX REPLACE
                    "[ \t]*${line}\n"
                    ""
                    common_cmake_contents
                    "${common_cmake_contents}"
                )
            endforeach()
            string(REGEX REPLACE
                "[ \t]*target_link_libraries\\(\\$\\{MICROPY_TARGET\\}[ \t]+micropy_extmod_btree\\)\n"
                ""
                common_cmake_contents
                "${common_cmake_contents}"
            )
            string(REPLACE
                "target_link_libraries(${MICROPY_TARGET} micropy_extmod_btree)"
                ""
                common_cmake_contents
                "${common_cmake_contents}"
            )
            file(WRITE ${common_cmake} "${common_cmake_contents}")
        endif()
    endforeach()

    if(TARGET __idf_mr9you__micropython-helper AND EXISTS ${board_dir})
        target_include_directories(__idf_mr9you__micropython-helper PUBLIC ${board_dir})
    endif()
endfunction()

include($ENV{IDF_PATH}/tools/cmake/project.cmake)

set(ENV{ESP_IDF_VERSION} "${IDF_VERSION_MAJOR}.${IDF_VERSION_MINOR}")

set(dependencies_lock "${CMAKE_CURRENT_LIST_DIR}/dependencies.lock")
set(components_lock "$ENV{PROJECT_DIR}/components_${IDF_TARGET}.lock")

if("${IDF_VERSION_MAJOR}.${IDF_VERSION_MINOR}" VERSION_GREATER_EQUAL "5.1")
    idf_build_set_property(DEPENDENCIES_LOCK ${components_lock})
else()
    if(EXISTS ${components_lock})
        file(COPY_FILE ${components_lock} ${dependencies_lock} ONLY_IF_DIFFERENT)
    else()
        file(REMOVE ${dependencies_lock})
    endif()
endif()

project(libespidf)

file(GLOB PHASE0_GENERATED_MICROPY_HELPERS
    "${CMAKE_BINARY_DIR}/managed_components/mr9you__micropython-helper"
    "${CMAKE_BINARY_DIR}/../managed_components/mr9you__micropython-helper"
    "${CMAKE_BINARY_DIR}/../../managed_components/mr9you__micropython-helper"
)
list(REMOVE_DUPLICATES PHASE0_GENERATED_MICROPY_HELPERS)
foreach(candidate ${PHASE0_GENERATED_MICROPY_HELPERS})
    phase0_force_patch_generated_micropy_helper(${candidate})
endforeach()

if(EXISTS ${MICROPY_HELPER_DIR})
    set(MICROPY_BOARD_DIR ${MICROPY_HELPER_DIR}/micropython/ports/esp32/boards/${MICROPY_BOARD})
    set(MICROPY_BOARD_HEADER ${MICROPY_BOARD_DIR}/mpconfigboard.h)
    set(MICROPY_ESP32_COMMON_CMAKE ${MICROPY_HELPER_DIR}/esp32_common.cmake)
    set(MICROPY_PORT_CONFIG ${MICROPY_HELPER_DIR}/micropython/ports/esp32/mpconfigport.h)
    set(MICROPY_STARTUP_SOURCE ${MICROPY_HELPER_DIR}/mpy_startup.c)
    set(MICROPY_USB_SERIAL_JTAG_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp32/usb_serial_jtag.c)
    set(MICROPY_MODESP32_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp32/modesp32.c)
    set(MICROPY_MODSOCKET_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp32/modsocket.c)
    set(MICROPY_MODESP_SOURCE ${MICROPY_HELPER_DIR}/micropython/ports/esp8266/modesp.c)
    set(MICROPY_MPYCROSS_BINARY ${MICROPY_HELPER_DIR}/micropython/mpy-cross/build/mpy-cross)
    if(EXISTS ${MICROPY_BOARD_HEADER})
        file(READ ${MICROPY_BOARD_HEADER} MICROPY_BOARD_HEADER_CONTENTS)
        string(REPLACE
            "#define MICROPY_HW_ENABLE_UART_REPL         (1)"
            "#define MICROPY_HW_ENABLE_UART_REPL         (0)"
            MICROPY_BOARD_HEADER_CONTENTS
            "${MICROPY_BOARD_HEADER_CONTENTS}"
        )
        file(WRITE ${MICROPY_BOARD_HEADER} "${MICROPY_BOARD_HEADER_CONTENTS}")
        configure_file(
            ${MICROPY_BOARD_HEADER}
            ${MICROPY_HELPER_DIR}/micropython/ports/esp32/mpconfigboard.h
            COPYONLY
        )
    endif()
    if(EXISTS ${MICROPY_ESP32_COMMON_CMAKE})
        file(READ ${MICROPY_ESP32_COMMON_CMAKE} MICROPY_ESP32_COMMON_CONTENTS)
        foreach(rewrite
            "    set(MICROPY_PY_BTREE ON)|    set(MICROPY_PY_BTREE OFF)"
            "    ${MICROPY_DIR}/shared/netutils/netutils.c|"
            "    ppp_set_auth.c|"
            "    network_common.c|"
            "    network_lan.c|"
            "    network_ppp.c|"
            "    network_wlan.c|"
            "    mpnimbleport.c|"
            "    modsocket.c|"
            "    modesp.c|"
            "    machine_sdcard.c|"
            "    machine_bitstream.c|"
            "    modespnow.c|"
            "    esp32_ulp.c|"
            "    esp32_rmt.c|"
            "    mpthreadport.c|"
            "    usb.c|"
            "    bt|"
            "    esp_eth|"
            "    esp_event|"
            "    esp_netif|"
            "    esp_ringbuf|"
            "    esp_wifi|"
            "    lwip|"
            "    sdmmc|"
            "    ulp|"
        )
            string(REPLACE "|" ";" rewrite_parts "${rewrite}")
            list(GET rewrite_parts 0 rewrite_from)
            list(GET rewrite_parts 1 rewrite_to)
            string(REPLACE "${rewrite_from}\n" "${rewrite_to}" MICROPY_ESP32_COMMON_CONTENTS "${MICROPY_ESP32_COMMON_CONTENTS}")
        endforeach()
        foreach(line
            "\\$\\{MICROPY_DIR\\}/shared/netutils/netutils\\.c"
            "ppp_set_auth\\.c"
            "network_common\\.c"
            "network_lan\\.c"
            "network_ppp\\.c"
            "network_wlan\\.c"
            "mpnimbleport\\.c"
            "modsocket\\.c"
            "modesp\\.c"
            "machine_sdcard\\.c"
            "machine_bitstream\\.c"
            "modespnow\\.c"
            "esp32_ulp\\.c"
            "esp32_rmt\\.c"
            "mpthreadport\\.c"
            "usb\\.c"
        )
            string(REGEX REPLACE
                "[ \t]*${line}\n"
                ""
                MICROPY_ESP32_COMMON_CONTENTS
                "${MICROPY_ESP32_COMMON_CONTENTS}"
            )
        endforeach()
        foreach(line
            "bt"
            "esp_eth"
            "esp_event"
            "esp_netif"
            "esp_ringbuf"
            "esp_wifi"
            "lwip"
            "sdmmc"
            "ulp"
        )
            string(REGEX REPLACE
                "[ \t]*${line}\n"
                ""
                MICROPY_ESP32_COMMON_CONTENTS
                "${MICROPY_ESP32_COMMON_CONTENTS}"
            )
        endforeach()
        string(REGEX REPLACE
            "[ \t]*target_link_libraries\\(\\$\\{MICROPY_TARGET\\}[ \t]+micropy_extmod_btree\\)\n"
            ""
            MICROPY_ESP32_COMMON_CONTENTS
            "${MICROPY_ESP32_COMMON_CONTENTS}"
        )
        file(WRITE ${MICROPY_ESP32_COMMON_CMAKE} "${MICROPY_ESP32_COMMON_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_PORT_CONFIG})
        file(READ ${MICROPY_PORT_CONFIG} MICROPY_PORT_CONFIG_CONTENTS)
        string(REPLACE
            "#include \"esp_wifi_types.h\""
            "/* phase0 micropython spike: wifi types disabled */"
            MICROPY_PORT_CONFIG_CONTENTS
            "${MICROPY_PORT_CONFIG_CONTENTS}"
        )
        foreach(rewrite
            "#define MICROPY_PY_THREAD                   (1)|#define MICROPY_PY_THREAD                   (0)"
            "#define MICROPY_PY_THREAD_GIL               (1)|#define MICROPY_PY_THREAD_GIL               (0)"
            "#define MICROPY_PY_ESPNOW                   (1)|#define MICROPY_PY_ESPNOW                   (0)"
            "#define MICROPY_PY_BLUETOOTH                (1)|#define MICROPY_PY_BLUETOOTH                (0)"
            "#define MICROPY_PY_NETWORK (1)|#define MICROPY_PY_NETWORK (0)"
            "#define MICROPY_PY_NETWORK_WLAN             (1)|#define MICROPY_PY_NETWORK_WLAN             (0)"
            "#define MICROPY_HW_ENABLE_SDCARD            (1)|#define MICROPY_HW_ENABLE_SDCARD            (0)"
            "#define MICROPY_PY_SSL                      (1)|#define MICROPY_PY_SSL                      (0)"
            "#define MICROPY_PY_WEBSOCKET                (1)|#define MICROPY_PY_WEBSOCKET                (0)"
            "#define MICROPY_PY_WEBREPL                  (1)|#define MICROPY_PY_WEBREPL                  (0)"
        )
            string(REPLACE "|" ";" rewrite_parts "${rewrite}")
            list(GET rewrite_parts 0 rewrite_from)
            list(GET rewrite_parts 1 rewrite_to)
            string(REPLACE "${rewrite_from}" "${rewrite_to}" MICROPY_PORT_CONFIG_CONTENTS "${MICROPY_PORT_CONFIG_CONTENTS}")
        endforeach()
        foreach(rewrite
            "MICROPY_PY_THREAD"
            "MICROPY_PY_THREAD_GIL"
            "MICROPY_PY_ESPNOW"
            "MICROPY_PY_BLUETOOTH"
            "MICROPY_PY_NETWORK"
            "MICROPY_PY_NETWORK_WLAN"
            "MICROPY_HW_ENABLE_SDCARD"
            "MICROPY_PY_SSL"
            "MICROPY_PY_WEBSOCKET"
            "MICROPY_PY_WEBREPL"
        )
            string(REGEX REPLACE
                "(#define[ \t]+${rewrite}[ \t]+)\\(1\\)"
                "\\1(0)"
                MICROPY_PORT_CONFIG_CONTENTS
                "${MICROPY_PORT_CONFIG_CONTENTS}"
            )
        endforeach()
        file(WRITE ${MICROPY_PORT_CONFIG} "${MICROPY_PORT_CONFIG_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_STARTUP_SOURCE})
        file(READ ${MICROPY_STARTUP_SOURCE} MICROPY_STARTUP_CONTENTS)
        foreach(rewrite
            "#include \"modnetwork.h\"|/* phase0 micropython spike: network disabled */"
            "#include \"mpthreadport.h\"|/* phase0 micropython spike: thread port disabled */"
            "#include \"usb.h\"|/* phase0 micropython spike: usb disabled */"
        )
            string(REPLACE "|" ";" rewrite_parts "${rewrite}")
            list(GET rewrite_parts 0 rewrite_from)
            list(GET rewrite_parts 1 rewrite_to)
            string(REPLACE "${rewrite_from}" "${rewrite_to}" MICROPY_STARTUP_CONTENTS "${MICROPY_STARTUP_CONTENTS}")
        endforeach()
        file(WRITE ${MICROPY_STARTUP_SOURCE} "${MICROPY_STARTUP_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_USB_SERIAL_JTAG_SOURCE})
        file(READ ${MICROPY_USB_SERIAL_JTAG_SOURCE} MICROPY_USB_SERIAL_JTAG_CONTENTS)
        string(REPLACE
            "#include \"freertos/portmacro.h\""
            "#include \"freertos/portmacro.h\"\n#ifndef USB_SERIAL_JTAG_PACKET_SZ_BYTES\n#define USB_SERIAL_JTAG_PACKET_SZ_BYTES 64\n#endif"
            MICROPY_USB_SERIAL_JTAG_CONTENTS
            "${MICROPY_USB_SERIAL_JTAG_CONTENTS}"
        )
        file(WRITE ${MICROPY_USB_SERIAL_JTAG_SOURCE} "${MICROPY_USB_SERIAL_JTAG_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_MODESP32_SOURCE})
        file(READ ${MICROPY_MODESP32_SOURCE} MICROPY_MODESP32_CONTENTS)
        string(REGEX REPLACE
            "[ \t]*\\{ MP_ROM_QSTR\\(MP_QSTR_ULP\\), MP_ROM_PTR\\(&esp32_ulp_type\\) \\},\n"
            ""
            MICROPY_MODESP32_CONTENTS
            "${MICROPY_MODESP32_CONTENTS}"
        )
        string(REGEX REPLACE
            "[ \t]*\\{ MP_ROM_QSTR\\(MP_QSTR_RMT\\), MP_ROM_PTR\\(&esp32_rmt_type\\) \\},\n"
            ""
            MICROPY_MODESP32_CONTENTS
            "${MICROPY_MODESP32_CONTENTS}"
        )
        file(WRITE ${MICROPY_MODESP32_SOURCE} "${MICROPY_MODESP32_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_MODSOCKET_SOURCE})
        file(READ ${MICROPY_MODSOCKET_SOURCE} MICROPY_MODSOCKET_CONTENTS)
        string(REPLACE
            "MP_REGISTER_EXTENSIBLE_MODULE(MP_QSTR_socket, mp_module_socket);"
            "/* phase0 micropython spike: socket registration disabled */"
            MICROPY_MODSOCKET_CONTENTS
            "${MICROPY_MODSOCKET_CONTENTS}"
        )
        file(WRITE ${MICROPY_MODSOCKET_SOURCE} "${MICROPY_MODSOCKET_CONTENTS}")
    endif()
    if(EXISTS ${MICROPY_MODESP_SOURCE})
        file(READ ${MICROPY_MODESP_SOURCE} MICROPY_MODESP_CONTENTS)
        string(REPLACE
            "MP_REGISTER_MODULE(MP_QSTR_esp, esp_module);"
            "/* phase0 micropython spike: esp registration disabled */"
            MICROPY_MODESP_CONTENTS
            "${MICROPY_MODESP_CONTENTS}"
        )
        file(WRITE ${MICROPY_MODESP_SOURCE} "${MICROPY_MODESP_CONTENTS}")
    endif()
    foreach(stub_source
        network_common.c
        network_lan.c
        network_ppp.c
        network_wlan.c
        mpnimbleport.c
        modsocket.c
        modesp.c
        machine_sdcard.c
        machine_bitstream.c
        modespnow.c
        esp32_ulp.c
        esp32_rmt.c
        mpthreadport.c
        ppp_set_auth.c
        usb.c
    )
        set(stub_path ${MICROPY_HELPER_DIR}/micropython/ports/esp32/${stub_source})
        if(EXISTS ${stub_path})
            file(WRITE ${stub_path} "/* phase0 micropython spike: disabled for embedded tool sandbox */\n")
        endif()
    endforeach()
    if(TARGET __idf_mr9you__micropython-helper)
        target_include_directories(__idf_mr9you__micropython-helper PUBLIC ${MICROPY_BOARD_DIR})
    endif()
    if(TARGET BUILD_FROZEN_CONTENT)
        add_custom_command(
            OUTPUT ${MICROPY_MPYCROSS_BINARY}
            COMMAND ${CMAKE_COMMAND} -E make_directory ${MICROPY_HELPER_DIR}/micropython/mpy-cross/build
            COMMAND ${CMAKE_COMMAND} -E env SDKROOT=/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk make -C ${MICROPY_HELPER_DIR}/micropython/mpy-cross -j4
            VERBATIM
        )
        add_custom_target(phase0_build_mpy_cross DEPENDS ${MICROPY_MPYCROSS_BINARY})
        add_dependencies(BUILD_FROZEN_CONTENT phase0_build_mpy_cross)
    endif()
endif()

if(EXISTS ${dependencies_lock})
    file(COPY_FILE ${dependencies_lock} ${components_lock} ONLY_IF_DIFFERENT)
endif()

idf_build_get_property(comps BUILD_COMPONENTS)
set(BUILD_COMPONENTS ${comps} CACHE STRING "all esp-idf components" FORCE)

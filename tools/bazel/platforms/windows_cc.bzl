"""Custom Windows MSVC CC toolchain that overrides msvc_env_tmp.

The buildbuddy_toolchain template hardcodes msvc_env_tmp to C:\\Users\\User\\...\\Temp
which doesn't exist on self-hosted Windows executors. This macro creates an
identical CC toolchain but with a writable temp path.
"""

load("@buildbuddy_toolchain//:windows_cc_toolchain_config.bzl", _cc_toolchain_config = "cc_toolchain_config")
load("@buildbuddy_toolchain//:msvc_config.bzl", "MSVC_BUILTIN_INCLUDE_PATHS")
load("@rules_cc//cc:defs.bzl", "cc_toolchain")

# These values must match the buildbuddy.msvc_toolchain() tag in MODULE.bazel.
_MSVC_EDITION = "Professional"
_MSVC_RELEASE = "2022"
_MSVC_VERSION = "14.42.34433"
_WINSDK_VERSION = "10.0.26100.0"
_MSVC_ROOT = "C:\\Program Files\\Microsoft Visual Studio\\{}\\{}".format(_MSVC_RELEASE, _MSVC_EDITION)
_WINSDK_ROOT = "C:\\Program Files (x86)\\Windows Kits\\10"

def meerkat_windows_cc_toolchain(name):
    config_name = name + "_config"
    toolchain_name = name + "_toolchain_impl"
    cc_name = name + "_cc_impl"

    _cc_toolchain_config(
        name = config_name,
        cpu = "x64_windows",
        compiler = "msvc-cl",
        host_system_name = "local",
        target_system_name = "local",
        target_libc = "msvcrt",
        abi_version = "local",
        abi_libc_version = "local",
        toolchain_identifier = config_name,
        msvc_env_tmp = "C:\\Windows\\Temp",
        msvc_env_path = ";".join([
            _MSVC_ROOT + "\\Common7\\IDE\\",
            _MSVC_ROOT + "\\VC\\Tools\\MSVC\\" + _MSVC_VERSION + "\\bin\\HostX64\\x64",
            _MSVC_ROOT + "\\MSBuild\\Current\\Bin\\amd64",
            _WINSDK_ROOT + "\\bin\\" + _WINSDK_VERSION + "\\x64",
        ]),
        msvc_env_include = ";".join([
            _WINSDK_ROOT + "\\Include\\" + _WINSDK_VERSION + "\\cppwinrt",
            _WINSDK_ROOT + "\\Include\\" + _WINSDK_VERSION + "\\shared",
            _WINSDK_ROOT + "\\Include\\" + _WINSDK_VERSION + "\\ucrt",
            _WINSDK_ROOT + "\\Include\\" + _WINSDK_VERSION + "\\um",
            _WINSDK_ROOT + "\\Include\\" + _WINSDK_VERSION + "\\winrt",
            _MSVC_ROOT + "\\VC\\Auxiliary\\VS\\include",
            _MSVC_ROOT + "\\VC\\Tools\\MSVC\\" + _MSVC_VERSION + "\\include",
        ]),
        msvc_env_lib = ";".join([
            _WINSDK_ROOT + "\\Lib\\" + _WINSDK_VERSION + "\\um\\x64",
            _WINSDK_ROOT + "\\Lib\\" + _WINSDK_VERSION + "\\ucrt\\x64",
            _MSVC_ROOT + "\\VC\\Tools\\MSVC\\" + _MSVC_VERSION + "\\lib\\x64",
        ]),
        msvc_cl_path = _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/cl.exe",
        msvc_ml_path = _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/ml64.exe",
        msvc_link_path = _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/link.exe",
        msvc_lib_path = _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/lib.exe",
        cxx_builtin_include_directories = MSVC_BUILTIN_INCLUDE_PATHS,
        tool_paths = {
            "ar": _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/lib.exe",
            "ml": _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/ml64.exe",
            "cpp": _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/cl.exe",
            "gcc": _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/cl.exe",
            "gcov": "wrapper/bin/msvc_nop.bat",
            "ld": _MSVC_ROOT + "/VC/Tools/MSVC/" + _MSVC_VERSION + "/bin/HostX64/x64/link.exe",
            "nm": "wrapper/bin/msvc_nop.bat",
            "objcopy": "wrapper/bin/msvc_nop.bat",
            "objdump": "wrapper/bin/msvc_nop.bat",
            "strip": "wrapper/bin/msvc_nop.bat",
        },
        archiver_flags = ["/MACHINE:X64"],
        default_link_flags = ["/MACHINE:X64"],
        dbg_mode_debug_flag = "/DEBUG:FULL",
        fastbuild_mode_debug_flag = "/DEBUG:FASTLINK",
        supports_parse_showincludes = True,
    )

    native.filegroup(name = name + "_empty", srcs = [])
    native.filegroup(name = name + "_compiler_files", srcs = ["@buildbuddy_toolchain//:msvc_config.bzl"])

    cc_toolchain(
        name = cc_name,
        toolchain_identifier = config_name,
        toolchain_config = ":" + config_name,
        all_files = ":" + name + "_empty",
        ar_files = ":" + name + "_empty",
        as_files = ":" + name + "_compiler_files",
        compiler_files = ":" + name + "_compiler_files",
        dwp_files = ":" + name + "_empty",
        linker_files = ":" + name + "_empty",
        objcopy_files = ":" + name + "_empty",
        strip_files = ":" + name + "_empty",
        supports_param_files = True,
    )

    native.toolchain(
        name = toolchain_name,
        exec_compatible_with = [
            "@platforms//cpu:x86_64",
            "@platforms//os:windows",
        ],
        target_compatible_with = [
            "@platforms//cpu:x86_64",
            "@platforms//os:windows",
        ],
        toolchain = ":" + cc_name,
        toolchain_type = "@bazel_tools//tools/cpp:toolchain_type",
    )

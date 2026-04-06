fn main() {
    embuild::espidf::sysenv::output();
    for key in [
        "WIFI_SSID",
        "WIFI_PASS",
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "OPENAI_STREAM",
        "OPENAI_STREAM_DEVICE",
        "OPENAI_STREAM_HOST",
        "PARK_ONLY",
        "ENABLE_COMMS",
        "SKIP_SINGLE_NODE",
        "HOST_PEER_NAME",
        "HOST_PEER_ADDR",
        "COMMS_LISTEN_PORT",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
}

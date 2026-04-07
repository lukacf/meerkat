fn main() {
    embuild::espidf::sysenv::output();
    for key in ["WIFI_SSID", "WIFI_PASS"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
}

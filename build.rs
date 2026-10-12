fn main() {
    println!("cargo:rerun-if-changed=assets/snapclip.rc");
    println!("cargo:rerun-if-changed=assets/snapclip.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut parts: Vec<&str> = version.split(['.', '-']).take(3).collect();
    parts.resize(3, "0");
    let comma = format!("VERSION_COMMA={},{},{},0", parts[0], parts[1], parts[2]);
    let string = format!("VERSION_STR=\"{version}\"");
    embed_resource::compile("assets/snapclip.rc", [comma, string])
        .manifest_optional()
        .expect("failed to embed Windows resources");
}

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile UI");
    // Slint declares only its own inputs, which stops cargo noticing a new icon by itself.
    println!("cargo:rerun-if-changed=ui/icon.ico");
    winresource::WindowsResource::new().set_icon_with_id("ui/icon.ico", "1").compile().expect("embed the app icon");
}

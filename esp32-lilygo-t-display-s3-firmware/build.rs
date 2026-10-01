fn main() {
    embuild::espidf::sysenv::output();

    // EmbedForSoftwareRenderer: pre-rasterizes fonts/images at compile
    // time into the binary -- required for the software renderer without
    // slint's "systemfonts" feature (which this project deliberately
    // doesn't enable, see Cargo.toml's comment on why). Without this, any
    // Text element panics at runtime with "No font fallback found."
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("src/ui/app.slint", config).expect("app.slint should compile");
}

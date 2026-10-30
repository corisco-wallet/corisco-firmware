fn main() {
    embuild::espidf::sysenv::output();

    // Fonts must be embedded: without slint's "systemfonts" feature, any Text panics with
    // "No font fallback found".
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("src/ui/app.slint", config).expect("app.slint should compile");
}

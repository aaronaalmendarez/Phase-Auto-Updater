fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='asInvoker'");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/PhaseAnimator.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Could not embed Windows app icon: {error}");
        }
    }
}

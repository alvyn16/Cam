fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Cam");
        res.set("FileDescription", "Cam Screen Recorder");
        if let Err(e) = res.compile() {
            eprintln!("Warning: failed to compile Windows resources: {e}");
        }
    }
}

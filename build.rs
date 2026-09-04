// English comments: embed Windows icon + VersionInfo (EXE identity).
// Runs only when HOST is Windows; other platforms skip silently.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/asislog.ico");
        res.set("FileDescription", "AsisLog - penampil log portabel");
        res.set("ProductName", "AsisLog");
        res.set("OriginalFilename", "asislog.exe");
        res.set("LegalCopyright", "MIT License");
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        if let Err(e) = res.compile() {
            eprintln!("warning: winres failed (EXE will lack icon): {}", e);
        }
    }
}

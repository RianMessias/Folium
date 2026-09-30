#[cfg(windows)]
fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    if let Err(e) = res.compile() {
        eprintln!("winresource falhou (icone do exe pode ausentar): {e}");
    }
}

#[cfg(not(windows))]
fn main() {}

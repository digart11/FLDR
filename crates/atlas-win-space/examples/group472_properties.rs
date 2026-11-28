#[cfg(windows)]
#[path = "../../atlas-ui/src/ui/context_metadata.rs"] mod context_metadata;
// Read-only native metadata audit; explicit files only, no scans.
#[cfg(windows)]
fn main() {
    for arg in std::env::args_os().skip(1) {
        let path=std::path::PathBuf::from(arg);
        println!("FILE {}",path.display());
        let properties=atlas_win_space::metadata::named_properties(&path);
        println!("EXTENDED {}",context_metadata::fields_at_level(path.extension().and_then(|s|s.to_str()).unwrap_or(""), &properties, atlas_model::settings::InformationLevel::Extended).join(" · "));
        for (key,label,value) in properties {
            println!("{key}\t{label}\t{value}");
        }
    }
}
#[cfg(not(windows))] fn main() {}

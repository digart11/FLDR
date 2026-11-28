//! Read-only, system-formatted properties. Call only on a background worker.
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyDescription, PSGetPropertyDescriptionByName, PDFF_DEFAULT,
};

const PROPERTIES: &[&str] = &[
    "System.ItemTypeText",
    "System.DateModified",
    "System.Image.HorizontalSize",
    "System.Image.VerticalSize",
    "System.Music.Artist",
    "System.Media.Duration",
    "System.Audio.EncodingBitrate",
    "System.Photo.CameraModel",
    "System.Photo.LensModel",
    "System.Photo.ISOSpeed",
    "System.Photo.ExposureTime",
    "System.Photo.FNumber",
    "System.Photo.FocalLength",
    "System.Photo.CameraManufacturer",
    "System.Video.EncodingBitrate",
    "System.Audio.SampleRate",
    "System.Audio.ChannelCount",
    "System.FileAttributes",
    "System.Photo.DateTaken",
    "System.Image.Dimensions",
    "System.Image.BitDepth",
    "System.Video.FrameWidth",
    "System.Video.FrameHeight",
    "System.Video.FrameRate",
    "System.Video.Compression",
    "System.Audio.Format",
    "System.Title",
    "System.Author",
    "System.Document.PageCount",
    "System.Document.WordCount",
];

use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToUInt32, PropVariantToUInt64, PropVariantToDouble, PropVariantToStringAlloc};
struct Property(PROPVARIANT);
impl Drop for Property { fn drop(&mut self) { unsafe { let _ = PropVariantClear(&mut self.0); } } }

/// Same units/order as Info Bar+: no parsing of localized display strings.
fn numeric_property(name: &str, n: f64) -> Option<String> {
    if !n.is_finite() || n <= 0.0 { return None; }
    match name {
        "System.Video.FrameRate" => {let s=format!("{:.2}",n/1000.);Some(format!("{} fps",s.trim_end_matches('0').trim_end_matches('.')))},
        "System.Photo.FNumber" => Some(format!("f/{n:.1}")),
        "System.Photo.ExposureTime" if n < 1.0 => Some(format!("1/{:.0}s",(1.0/n).round().max(1.0))),
        "System.Photo.ExposureTime" => Some(format!("{n:.1}s")),
        "System.Photo.FocalLength" => Some(format!("{n:.0}mm")),
        "System.Audio.SampleRate" if n >= 1000.0 => Some(format!("{:.1} kHz",n/1000.)),
        "System.Audio.SampleRate" => Some(format!("{n:.0} Hz")),
        _ => Some(format!("{n:.0}")),
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

// Both property formatting and display-name APIs allocate with CoTaskMemAlloc.
unsafe fn take_string(value: PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    text
}

/// Missing handlers and absent properties are ordinary empty results. Localized
/// labels, dates, durations, bitrates and units come from Windows, not Atlas.
pub fn extended_properties(path: &Path) -> Vec<(String, String)> {
    named_properties(path).into_iter().map(|(_, label, value)| (label, value)).collect()
}

/// Canonical names let compact consumers select fields independently of UI language.
pub fn named_properties(path: &Path) -> Vec<(String, String, String)> {
    if crate::directory::is_cloud_placeholder(path) { return Vec::new(); }
    let mut properties = windows_properties(path);
    // Adobe's fixed 26-byte PSD/PSB header, reviewed against psd-tools FileHeader.
    // No image decoding, resources, layers or variable-length allocation.
    let psd = path.extension().and_then(|v| v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case("psd") || v.eq_ignore_ascii_case("psb"));
    let local = path.to_str().is_some_and(|v| v.as_bytes().get(1) == Some(&b':'));
    if psd && local && !properties.iter().any(|(key, _, _)| key == "System.Image.Dimensions") {
        use std::io::Read;
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
            let mut header = [0u8; 26];
            if std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut header)).is_ok() {
                if let Some((width, height)) = psd_dimensions(&header) {
                    properties.push(("System.Image.Dimensions".into(), "Dimensions (PSD header)".into(), format!("{width} × {height}")));
                }
            }
        }
    }
    properties
}
fn psd_dimensions(h: &[u8]) -> Option<(u32,u32)> {
    if h.len() != 26 || &h[..4] != b"8BPS" || h[6..12] != [0;6] { return None; }
    let version = u16::from_be_bytes(h[4..6].try_into().ok()?);
    let max = match version { 1 => 30_000, 2 => 300_000, _ => return None };
    let channels = u16::from_be_bytes(h[12..14].try_into().ok()?);
    let height = u32::from_be_bytes(h[14..18].try_into().ok()?);
    let width = u32::from_be_bytes(h[18..22].try_into().ok()?);
    let depth = u16::from_be_bytes(h[22..24].try_into().ok()?);
    let color = u16::from_be_bytes(h[24..26].try_into().ok()?);
    ((1..=56).contains(&channels) && (1..=max).contains(&height) && (1..=max).contains(&width)
        && matches!(depth, 1|8|16|32) && matches!(color, 0|1|2|3|4|7|8|9)).then_some((width,height))
}
fn windows_properties(path: &Path) -> Vec<(String, String, String)> {
    if crate::directory::is_cloud_placeholder(path) {
        return Vec::new();
    }
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        // Tokio's blocking pool is normally uninitialized. If a caller already
        // owns an STA, use it without balancing an initialization that failed.
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _apartment = initialized.is_ok().then_some(Apartment);
        if initialized.is_err() && initialized != windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            return Vec::new();
        }
        // Match Info Bar+ IShellItem2::GetProperty access rather than a
        // GPS_DEFAULT store whose formatted value can omit valid native data.
        let Ok(item): windows::core::Result<windows::Win32::UI::Shell::IShellItem2> =
            windows::Win32::UI::Shell::SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) else { return Vec::new(); };
        let audit = std::env::var_os("FILEOP_METADATA_AUDIT").is_some();
        PROPERTIES
            .iter()
            .filter_map(|name| {
                let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
                let mut raw = std::ptr::null_mut();
                PSGetPropertyDescriptionByName(
                    PCWSTR(wide.as_ptr()),
                    &IPropertyDescription::IID,
                    &mut raw,
                )
                .ok()?;
                if raw.is_null() {
                    return None;
                }
                let description = IPropertyDescription::from_raw(raw);
                let mut key = windows::Win32::Foundation::PROPERTYKEY::default();
                description.GetPropertyKey(&mut key).ok()?;
                // Exactly one property read per key. All conversion/formatting
                // below consumes that same PROPVARIANT, never re-queries a store.
                let raw_value = match item.GetProperty(&key) {
                    Ok(value) => Property(value),
                    Err(error) => { if audit { eprintln!("{name} key={:?}/{} GetProperty={:?}",key.fmtid,key.pid,error.code()); } return None; }
                };
                let vt = raw_value.0.Anonymous.Anonymous.vt;
                if audit { eprintln!("{name} key={:?}/{} GetProperty=S_OK vt={}",key.fmtid,key.pid,vt.0); }
                use windows::Win32::System::Variant::{VT_EMPTY, VT_NULL};
                if vt == VT_EMPTY || vt == VT_NULL { return None; }
                let pv = &raw_value.0;
                let value = match *name {
                    "System.Media.Duration" => {
                        let ticks = PropVariantToUInt64(pv).ok()?; if ticks == 0 {return None;}
                        let seconds=ticks/10_000_000;format!("{:02}:{:02}:{:02}",seconds/3600,(seconds/60)%60,seconds%60)
                    },
                    "System.Image.HorizontalSize" | "System.Image.VerticalSize" | "System.Video.FrameWidth" | "System.Video.FrameHeight" | "System.Video.FrameRate" | "System.Photo.ISOSpeed" | "System.Audio.SampleRate" | "System.Audio.ChannelCount" => numeric_property(name, f64::from(PropVariantToUInt32(pv).ok()?))?,
                    "System.Photo.FNumber" | "System.Photo.ExposureTime" | "System.Photo.FocalLength" => numeric_property(name, PropVariantToDouble(pv).ok()?)?,
                    "System.Photo.CameraModel" | "System.Photo.LensModel" | "System.Photo.CameraManufacturer" => take_string(PropVariantToStringAlloc(pv).ok()?),
                    _ => take_string(description.FormatForDisplay(pv, PDFF_DEFAULT).ok()?),
                };
                if audit { eprintln!("{name} value={value}"); }
                if value.trim().is_empty() {
                    return None;
                }
                let label = description
                    .GetDisplayName()
                    .ok()
                    .map(|v| take_string(v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| (*name).to_string());
                Some(((*name).to_string(), label, value))
            })
            .collect()
    }
}

/// Read the stored filesystem destination only. Never call Resolve, stat the
/// target, invoke link tracking, or open the destination. Blocking worker only.
pub fn shortcut_target(path: &Path) -> Option<String> {
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _apartment = initialized.is_ok().then_some(Apartment);
        if initialized.is_err() && initialized != windows::Win32::Foundation::RPC_E_CHANGED_MODE { return None; }
        use windows::Win32::{System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER}, UI::Shell::{IShellLinkW, ShellLink}};
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persistence: windows::Win32::System::Com::IPersistFile = link.cast().ok()?;
        persistence.Load(windows::core::PCWSTR(path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>().as_ptr()), windows::Win32::System::Com::STGM_READ).ok()?;
        let mut buffer = vec![0u16; 32768];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0).ok()?;
        let end = buffer.iter().position(|c| *c == 0)?;
        let target = String::from_utf16(&buffer[..end]).ok()?;
        atlas_model::file_presentation::path_text(Path::new(&target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_targets_local_unc_broken_and_malformed() {
        use windows::Win32::{System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, IPersistFile}, UI::Shell::{IShellLinkW, ShellLink}};
        let path = std::env::temp_dir().join(format!("fileop-group46-{}.lnk", std::process::id()));
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
        let _fixture = Fixture(path.clone());
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            let _apartment = Apartment;
            // Destinations deliberately need not exist. Never probe the network.
            for target in ["C:\\FileOP missing target\\\u{65e5}\u{672c}\u{8a9e} space.zxc", r"\\fileop-nonexistent-server\share\folder\file.txt", r"C:\"] {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                let wide_target: Vec<_> = target.encode_utf16().chain(Some(0)).collect();
                link.SetPath(PCWSTR(wide_target.as_ptr())).unwrap();
                let persistence: IPersistFile = link.cast().unwrap();
                let wide_path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                persistence.Save(PCWSTR(wide_path.as_ptr()), true).unwrap();
                assert_eq!(shortcut_target(&path).as_deref(), Some(target));
            }
        }
        std::fs::write(&path, b"malformed shortcut").unwrap();
        assert!(shortcut_target(&path).is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(shortcut_target(&path).is_none());
    }

    #[test]
    fn windows_formats_real_wave_duration_and_bitrate() {
        let path =
            std::env::temp_dir().join(format!("atlas-properties-{}.wav", std::process::id()));
        // One second of mono, 8-bit PCM at 8 kHz.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(8036u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.resize(8044, 128);
        std::fs::write(&path, wav).unwrap();
        let properties = extended_properties(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(properties.len() >= 2, "{properties:?}");
        assert!(properties
            .iter()
            .all(|(key, value)| !key.is_empty() && !value.is_empty()));
        assert!(
            properties
                .iter()
                .any(|(_, value)| value.contains("00:00:01")),
            "{properties:?}"
        );
    }

    #[test]
    fn windows_property_handlers_read_real_mp3_and_mp4_fixtures() {
        for (extension, bytes) in [
            ("mp3", include_bytes!("../tests/fixtures/metadata.mp3").as_slice()),
            ("mp4", include_bytes!("../tests/fixtures/metadata.mp4").as_slice()),
        ] {
            let path = std::env::temp_dir().join(format!("atlas-properties-{}.{extension}", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            let properties = extended_properties(&path);
            std::fs::remove_file(&path).unwrap();
            let has = |needle: &str| properties.iter().any(|(_, value)| value.contains(needle));
            assert!(has("00:00:01"), "{extension}: {properties:?}");
            if extension == "mp3" {
                assert!(has("Atlas Metadata Fixture"), "{properties:?}");
                assert!(has("128"), "{properties:?}");
            } else {
                assert!(has("32") && has("24"), "{properties:?}");
            }
        }
    }

    #[test]
    fn missing_file_has_no_properties() {
        assert!(extended_properties(Path::new(
            r"C:\atlas-missing-properties-fixture\missing.mp3"
        ))
        .is_empty());
    }
}

#[cfg(test)]
mod header_tests {
    use super::*;
    #[test] fn psd_dimensions_reach_the_native_provider_without_decoding() {
        let path = std::env::temp_dir().join(format!("fileop-group471-{}.psd",std::process::id()));
        struct Fixture(std::path::PathBuf);impl Drop for Fixture { fn drop(&mut self){let _=std::fs::remove_file(&self.0);} }
        let _fixture=Fixture(path.clone());
        let mut h=[0u8;26];h[..4].copy_from_slice(b"8BPS");h[4..6].copy_from_slice(&1u16.to_be_bytes());h[12..14].copy_from_slice(&3u16.to_be_bytes());h[14..18].copy_from_slice(&4000u32.to_be_bytes());h[18..22].copy_from_slice(&6000u32.to_be_bytes());h[22..24].copy_from_slice(&8u16.to_be_bytes());h[24..26].copy_from_slice(&3u16.to_be_bytes());
        std::fs::write(&path,h).unwrap();
        assert!(named_properties(&path).iter().any(|(k,_,v)| k=="System.Image.Dimensions" && v.contains("6000") && v.contains("4000")));
        h[0]=0;std::fs::write(&path,h).unwrap();
        assert!(!named_properties(&path).iter().any(|(k,_,_)| k=="System.Image.Dimensions"));
    }
    #[test] fn bounded_psd_header_rejects_malformed_and_limits() {
        let mut h=[0u8;26];h[..4].copy_from_slice(b"8BPS");h[4..6].copy_from_slice(&1u16.to_be_bytes());h[12..14].copy_from_slice(&3u16.to_be_bytes());h[14..18].copy_from_slice(&4000u32.to_be_bytes());h[18..22].copy_from_slice(&6000u32.to_be_bytes());h[22..24].copy_from_slice(&8u16.to_be_bytes());h[24..26].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(psd_dimensions(&h),Some((6000,4000)));
        for n in 0..26 { assert!(psd_dimensions(&h[..n]).is_none()); }
        h[18..22].copy_from_slice(&30001u32.to_be_bytes());assert!(psd_dimensions(&h).is_none());
        h[4..6].copy_from_slice(&2u16.to_be_bytes());assert_eq!(psd_dimensions(&h),Some((30001,4000)));
        h[6]=1;assert!(psd_dimensions(&h).is_none());
        for n in 0..=255u8 { assert!(psd_dimensions(&[n;26]).is_none()); }
    }
}

#[cfg(test)] mod typed_format_tests {
    use super::*;
    #[test] fn info_bar_numeric_units_do_not_use_rounded_shell_text() {
        assert_eq!(numeric_property("System.Video.FrameRate",23976.).as_deref(),Some("23.98 fps"));
        assert_eq!(numeric_property("System.Photo.FNumber",9.).as_deref(),Some("f/9.0"));
        assert_eq!(numeric_property("System.Photo.ExposureTime",0.0125).as_deref(),Some("1/80s"));
        assert_eq!(numeric_property("System.Photo.ExposureTime",0.6).as_deref(),Some("1/2s"));
        assert_eq!(numeric_property("System.Photo.FocalLength",35.).as_deref(),Some("35mm"));
        assert_eq!(numeric_property("System.Audio.SampleRate",44100.).as_deref(),Some("44.1 kHz"));
        assert_eq!(numeric_property("System.Photo.ISOSpeed",1000.).as_deref(),Some("1000"));
        for n in [0.,-1.,f64::NAN,f64::INFINITY] { assert!(numeric_property("System.Photo.FNumber",n).is_none()); }
    }
}

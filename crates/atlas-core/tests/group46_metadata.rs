//! Exercise the existing metadata presentation without linking the UI renderer.
#[path = "../../atlas-ui/src/ui/context_metadata.rs"]
mod context_metadata;

#[cfg(windows)]
#[test]
fn existing_copy_path_action_formats_single_absolute_paths_as_plain_text() {
    for path in [r"C:\", r"C:\space dir\日本語.zxc", r"\\server\share\space dir\file.txt"] {
        assert_eq!(atlas_core::util::display::clipboard_path_text(&[path.into()]), path);
    }
}

#[test]
fn photo_tooltip_preserves_exposure_and_omits_missing_values() {
    assert_eq!(context_metadata::display_property("System.Photo.ExposureTime", "1/250 sec.").as_deref(), Some("1/250 sec."));
    assert_eq!(context_metadata::display_property("System.Photo.CameraModel", "Camera model").as_deref(), Some("Camera model"));
    assert!(context_metadata::display_property("System.Photo.ISOSpeed", "unknown").is_none());
    assert!(context_metadata::compact_fields("jpg", &[]).is_empty());
}

#[test]
fn expanded_photo_audio_and_video_properties_remain_available_without_fabrication() {
    for (key, value) in [("System.Photo.FNumber", "f/2.8"), ("System.Photo.FocalLength", "85 mm"),
        ("System.Photo.LensModel", "85mm lens"), ("System.Video.EncodingBitrate", "18 Mbps"),
        ("System.Audio.SampleRate", "48 kHz"), ("System.Audio.ChannelCount", "2") ] {
        assert_eq!(context_metadata::display_property(key, value).as_deref(), Some(value));
        assert!(context_metadata::display_property(key, "unknown").is_none());
    }
}

//! Pure presentation regressions, independent of the UI renderer.
#[path = "../../atlas-ui/src/ui/context_metadata.rs"] mod context_metadata;
#[path = "../../atlas-ui/src/ui/folder_stats_presentation.rs"] mod folder_stats_presentation;
#[path = "../../atlas-ui/src/ui/grid_presentation.rs"] mod grid_presentation;
#[path = "../../atlas-ui/src/ui/address_policy.rs"] mod address_policy;
#[path = "../../atlas-ui/src/ui/tab_shape.rs"] mod tab_shape;
use atlas_model::{folder_stats::SizeCell,settings::InformationLevel};
fn fields(ext:&str, items:&[(&str,&str)]) -> String {
    let p:Vec<_>=items.iter().map(|(k,v)|(k.to_string(),k.to_string(),v.to_string())).collect();
    context_metadata::fields_at_level(ext,&p,InformationLevel::Extended).join(" · ")
}
#[test] fn extended_matches_reference_field_order_without_duplicate_extension_or_iso() {
    assert_eq!(fields("CR2", &[("System.Image.Dimensions","5634 x 3752"),("System.Photo.CameraModel","EOS 5D Mark II"),("System.Photo.ISOSpeed","1000"),("System.Photo.FNumber","f/9.0"),("System.Photo.ExposureTime","1/80s"),("System.Photo.FocalLength","35mm")]),"5634×3752 · EOS 5D Mark II · ISO 1000 · f/9.0 · 1/80s · 35mm");
    assert_eq!(fields("mkv", &[("System.Video.FrameWidth","640"),("System.Video.FrameHeight","267"),("System.Media.Duration","01:38:30"),("System.Video.FrameRate","23.98 fps")]),"640×267 · 01:38:30 · 23.98 fps");
    assert_eq!(fields("jpg", &[("System.Image.Dimensions","3000 x 1833"),("System.Photo.CameraModel","Canon EOS 5D Mark IV"),("System.Photo.LensModel","EF16-35mm f/2.8L II USM"),("System.Photo.ISOSpeed","ISO-100"),("System.Photo.FNumber","f/7.1"),("System.Photo.ExposureTime","1/2s"),("System.Photo.FocalLength","20mm")]),"3000×1833 · Canon EOS 5D Mark IV · EF16-35mm f/2.8L II USM · ISO 100 · f/7.1 · 1/2s · 20mm");
    assert_eq!(fields("psd", &[("System.Image.Dimensions","6000 × 4000")]),"6000×4000");
}
#[test] fn cached_sizes_are_clean_without_changing_state_or_lower_bounds() {
    for n in [0,42,460900000,1024*1024*1024] {
        assert_eq!(folder_stats_presentation::text(SizeCell::Cached(n)),folder_stats_presentation::text(SizeCell::Exact(n)));
        assert!(!folder_stats_presentation::text(SizeCell::Cached(n)).contains('◷'));
        assert!(folder_stats_presentation::subdued(SizeCell::Cached(n)));
    }
    assert!(folder_stats_presentation::text(SizeCell::Partial(42)).starts_with('≥'));
}
#[test] fn badges_scale_with_icon_zoom_and_stay_inside_folder_lower_middle() {
    for z in [0.75,1.,1.25,1.5,2.] {
        assert!(grid_presentation::badge_metrics(63.*z,z).is_none());
        let a=grid_presentation::badge_metrics(64.*z,z).unwrap();let b=grid_presentation::badge_metrics(110.*z,z).unwrap();
        assert!(a.font == 12.5*z && b.font == a.font);
        for m in [a,b] { assert!(m.padding>=3.*z);assert!(m.offset+m.height<=if m.font==a.font {32.*z} else {55.*z}); }
        assert_eq!(grid_presentation::badge_metrics(110.,1.).unwrap().font*z,b.font);
    }
}
#[test] fn focused_child_drives_active_material_without_affecting_other_splits() {
    assert_eq!(tab_shape::active_child(&[1,2],Some(1),2),Some(2));
    assert_eq!(tab_shape::active_child(&[3,4],Some(3),2),Some(3));
    assert_eq!(tab_shape::active_child(&[1,2],Some(9),8),Some(1));
    assert_eq!(tab_shape::active_child::<u32>(&[],None,1),None);
}
#[test] fn inactive_materials_remain_recessed_on_hover_with_one_coat_only() {
    for alpha in [0.,0.035,0.4,0.8,1.] { for base in [0.02,0.5,0.95] { for hover in [0.,0.5,1.] {
        let reduction=address_policy::inactive_reduction(hover);assert!(reduction>=0.20);
        let dim=address_policy::darker_material([base,base,base,alpha],reduction);
        for backing in [0.02,0.5,0.95] {
            let active=base*alpha+backing*(1.-alpha);
            let inactive=dim[0]*dim[3]+backing*(1.-dim[3]);
            assert!((inactive-active*(1.-reduction)).abs()<0.00001);
        }
    } } }
}

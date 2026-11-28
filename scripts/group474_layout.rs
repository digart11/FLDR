// Standalone regression against the existing debug Taffy artifact; no UI launch.
use taffy::prelude::*;
#[path = "../crates/atlas-ui/src/ui/status_layout.rs"] mod status_layout;
#[test]
fn intrinsic_metadata_shrinks_only_when_necessary_and_never_displaces_essentials() {
    for pane_width in [280., 480., 640., 900., 1600.] {
        for focused in [false,true] {
            let placement = status_layout::placement(pane_width,focused);
            let available = pane_width - placement.right - 8.;
            // Empty/delayed -> Standard -> Extended -> unsupported -> completion.
            for text_width in [0., 120., 900., 0., 120.] {
                let mut tree = TaffyTree::<f32>::new();
                let fixed = |width| Style { size: Size {width: Dimension::length(width),height:Dimension::length(30.)}, flex_shrink:0., ..Default::default() };
                let counts = tree.new_leaf(fixed(80.)).unwrap();
                let extension = tree.new_leaf(fixed(40.)).unwrap();
                let mut children = vec![counts,extension];
                let metadata = if text_width > 0. {
                    let leaf = tree.new_leaf_with_context(Style { flex_grow:0., flex_shrink:1., flex_basis:Dimension::auto(), min_size:Size {width:Dimension::length(0.),height:Dimension::length(30.)}, ..Default::default() },text_width).unwrap();
                    children.push(leaf); Some(leaf)
                } else { None };
                let row = tree.new_with_children(Style {display:Display::Flex, size:Size {width:Dimension::length(available),height:Dimension::length(30.)},gap:Size {width:LengthPercentage::length(4.),height:LengthPercentage::length(0.)}, ..Default::default()},&children).unwrap();
                tree.compute_layout_with_measure(row,Size {width:AvailableSpace::Definite(available),height:AvailableSpace::Definite(30.)}, |known,_,_,context,_| Size {width:known.width.unwrap_or(context.copied().unwrap_or(0.)),height:known.height.unwrap_or(30.)}).unwrap();
                assert_eq!(tree.layout(counts).unwrap().size.width,80.);
                assert_eq!(tree.layout(extension).unwrap().size.width,40.);
                if let Some(metadata) = metadata {
                    let bounds=tree.layout(metadata).unwrap();
                    let expected=text_width.min(available-128.);
                    assert!((bounds.size.width-expected).abs()<=1.);
                    assert!(bounds.location.x+bounds.size.width<=available+1.);
                    if text_width < available-128. {assert!(bounds.size.width < available-120.);}
                } else {assert_eq!(children.len(),2);}
            }
        }
    }
}

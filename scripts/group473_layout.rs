// Standalone Taffy regression; compile against the existing debug Taffy artifact.
use taffy::prelude::*;
#[path = "../crates/atlas-ui/src/ui/status_layout.rs"] mod status_layout;
fn box_style(w: Dimension,h: Dimension)->Style { Style {size:Size{width:w,height:h}, ..Default::default()} }
#[test]
fn normal_flow_wrapper_reproduces_disappearance_but_absolute_anchor_survives_metadata_and_selection() {
 for width in [280.,480.,640.,900.,1600.] { for height in [300.,700.] { for focused in [false,true] {
  for (selection,metadata) in [(0,false),(1,false),(1,true),(3,false),(0,false),(1,true),(1,false)] {
   for broken in [true,false] {
    let mut t=TaffyTree::<()>::new();
    let content=t.new_leaf(box_style(Dimension::percent(1.),Dimension::percent(1.))).unwrap();
    let counts=t.new_leaf(box_style(Dimension::length(if selection>0 {180.}else{80.}),Dimension::length(30.))).unwrap();
    let mut children=vec![counts];
    if metadata {children.push(t.new_leaf(Style{flex_grow:1.,flex_basis:Dimension::length(0.),min_size:Size{width:Dimension::length(0.),height:Dimension::length(0.)},size:Size{width:Dimension::auto(),height:Dimension::length(30.)},..Default::default()}).unwrap());}
    let p=status_layout::placement(width,focused);
    let row=t.new_with_children(Style{position:Position::Absolute,inset:Rect{left:LengthPercentageAuto::length(8.),right:LengthPercentageAuto::length(p.right),bottom:LengthPercentageAuto::length(p.bottom),top:LengthPercentageAuto::auto()},..Default::default()},&children).unwrap();
    let overlay=if broken {t.new_with_children(box_style(Dimension::percent(1.),Dimension::percent(1.)),&[row]).unwrap()}else{row};
    // GPUI div defaults to a block layout; its children occupy normal flow.
    let root=t.new_with_children(Style{display:Display::Block,..box_style(Dimension::length(width),Dimension::length(height))},&[content,overlay]).unwrap();
    t.compute_layout(root,Size{width:AvailableSpace::Definite(width),height:AvailableSpace::Definite(height)}).unwrap();
    let row_bounds=t.layout(row).unwrap();
    let offset=if broken {t.layout(overlay).unwrap().location.y}else{0.};
    let bottom=offset+row_bounds.location.y+row_bounds.size.height;
    if broken {assert!(bottom>height,"old wrapper must leave pane");}
    else {assert!(bottom<=height && row_bounds.location.y>=0.);assert!(t.layout(counts).unwrap().size.width>0.);}
   }
  }
 } } }
}

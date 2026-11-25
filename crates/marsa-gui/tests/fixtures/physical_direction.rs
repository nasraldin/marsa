// .border_r_1(), TextAlign::Right
fn sample(left: i32, right: i32) {
    let _description = r#".text_left().margin_right(); Anchor::BottomLeft"#;
    let _comparison = left..right;
    let _keyboard = MouseButton::Left;

    div().pl_2().r#right(px(4.)).justify_end().text_align(TextAlign::Left);
    Styled::rounded_tr_lg(div());
    show(Anchor::TopRight);
    show(Anchor::LeftCenter);
    show(Anchor::RightCenter);
    let _edges = gpui::Edges { top: left, left, right: 0, bottom: 0 };
    let _data = Other { left, right };
}

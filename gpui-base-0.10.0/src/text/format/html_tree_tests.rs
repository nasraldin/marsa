use super::html_tree::{HtmlTree, Node, NodeData};
use html5ever::{ParseOpts, parse_document, tendril::TendrilSink};

fn parse(input: &str) -> HtmlTree {
    parse_document(HtmlTree::default(), ParseOpts::default()).one(input)
}

fn elements(node: &Node) -> Vec<String> {
    node.children
        .borrow()
        .iter()
        .flat_map(|node| match &node.data {
            NodeData::Element { name, .. } => {
                let mut names = vec![name.local.to_string()];
                names.extend(elements(node));
                names
            }
            _ => elements(node),
        })
        .collect()
}

#[test]
fn recovers_malformed_table_without_losing_inserted_nodes() {
    let tree = parse("<table><b>text</b><tr><td>A<td>B</table>");
    assert_eq!(
        elements(&tree.document),
        ["html", "head", "body", "b", "table", "tbody", "tr", "td", "td"],
    );
}

#[test]
fn template_contents_have_a_separate_fragment() {
    let tree = parse("<template><p>Hidden</p></template><p>Visible</p>");
    assert_eq!(elements(&tree.document), ["html", "head", "template", "body", "p"]);
}

#[test]
fn foreign_content_preserves_html_integration_points() {
    let tree = parse("<math><annotation-xml encoding='text/html'><p>A</p></annotation-xml></math>");
    assert_eq!(elements(&tree.document), ["html", "head", "body", "math", "annotation-xml", "p"]);
}

#[test]
fn nested_tree_drops_without_recursing_on_children() {
    let input = "<div>".repeat(12_000);
    drop(parse(&input));
}

//! The limited HTML tree used by the rich text importer and minifier.

use std::{
    borrow::Cow,
    cell::RefCell,
    io,
    rc::{Rc, Weak},
};

use html5ever::{
    Attribute, QualName,
    tendril::StrTendril,
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
};

pub(super) const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_TREE_DEPTH: usize = 128;
const MAX_TREE_NODES: usize = 50_000;

type Handle = Rc<Node>;

pub(super) struct Node {
    pub(super) children: RefCell<Vec<Handle>>,
    pub(super) data: NodeData,
    parent: RefCell<Weak<Node>>,
}

pub(super) enum NodeData {
    Document,
    Doctype,
    Text {
        contents: RefCell<StrTendril>,
    },
    Comment {
        contents: StrTendril,
    },
    ProcessingInstruction,
    Element {
        name: QualName,
        attrs: RefCell<Vec<Attribute>>,
        template: Option<Handle>,
        mathml_annotation: bool,
    },
}

impl Node {
    fn new(data: NodeData) -> Handle {
        Rc::new(Self {
            children: RefCell::default(),
            data,
            parent: RefCell::default(),
        })
    }

    fn insert(parent: &Handle, position: usize, node: NodeOrText<Handle>) {
        let mut children = parent.children.borrow_mut();
        let child = match node {
            NodeOrText::AppendNode(node) => node,
            NodeOrText::AppendText(text) => {
                if position > 0
                    && let NodeData::Text { contents } = &children[position - 1].data
                {
                    contents.borrow_mut().push_tendril(&text);
                    return;
                }
                Self::new(NodeData::Text {
                    contents: RefCell::new(text),
                })
            }
        };
        *child.parent.borrow_mut() = Rc::downgrade(parent);
        children.insert(position, child);
    }

    fn detach(node: &Handle) {
        if let Some(parent) = node.parent.take().upgrade() {
            parent
                .children
                .borrow_mut()
                .retain(|child| !Rc::ptr_eq(child, node));
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // Release deeply nested documents iteratively. Retained node handles
        // keep their descendants intact.
        let mut pending = self.children.take();
        if let NodeData::Element { template, .. } = &mut self.data {
            pending.extend(template.take());
        }
        while let Some(child) = pending.pop() {
            if let Ok(mut child) = Rc::try_unwrap(child) {
                pending.extend(child.children.take());
                if let NodeData::Element { template, .. } = &mut child.data {
                    pending.extend(template.take());
                }
            }
        }
    }
}

pub(super) struct HtmlTree {
    pub(super) document: Handle,
}

impl HtmlTree {
    /// Check the tree before entering recursive import and minification code.
    pub(super) fn check_limits(&self) -> io::Result<()> {
        let mut pending = vec![(self.document.clone(), 0)];
        let mut visited = 0;
        while let Some((node, depth)) = pending.pop() {
            visited += 1;
            if depth > MAX_TREE_DEPTH || visited + pending.len() > MAX_TREE_NODES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HTML tree exceeds its limits",
                ));
            }
            let children = node.children.borrow();
            if visited + pending.len() + children.len() > MAX_TREE_NODES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HTML tree exceeds its limits",
                ));
            }
            pending.extend(children.iter().cloned().map(|child| (child, depth + 1)));
            if let NodeData::Element {
                template: Some(template),
                ..
            } = &node.data
            {
                pending.push((template.clone(), depth + 1));
            }
        }
        Ok(())
    }
}

impl Default for HtmlTree {
    fn default() -> Self {
        Self {
            document: Node::new(NodeData::Document),
        }
    }
}

impl TreeSink for HtmlTree {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        Rc::clone(&self.document)
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> &'a QualName {
        match &target.data {
            NodeData::Element { name, .. } => name,
            // TreeSink guarantees that this method receives an element.
            _ => unreachable!("HTML tree builder requested a non-element name"),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        Node::new(NodeData::Element {
            name,
            attrs: RefCell::new(attrs),
            template: flags.template.then(|| Node::new(NodeData::Document)),
            mathml_annotation: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, contents: StrTendril) -> Handle {
        Node::new(NodeData::Comment { contents })
    }

    fn create_pi(&self, _: StrTendril, _: StrTendril) -> Handle {
        Node::new(NodeData::ProcessingInstruction)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        let length = parent.children.borrow().len();
        Node::insert(parent, length, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        previous: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.parent.borrow().upgrade().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(previous, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {
        self.append(
            &self.document,
            NodeOrText::AppendNode(Node::new(NodeData::Doctype)),
        );
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        match &target.data {
            NodeData::Element {
                template: Some(contents),
                ..
            } => Rc::clone(contents),
            // TreeSink guarantees that this method receives a template.
            _ => unreachable!("HTML tree builder requested contents from a non-template"),
        }
    }

    fn same_node(&self, first: &Handle, second: &Handle) -> bool {
        Rc::ptr_eq(first, second)
    }

    // Import and minification use structure rather than layout mode.
    fn set_quirks_mode(&self, _: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &Handle, node: NodeOrText<Handle>) {
        if let NodeOrText::AppendNode(child) = &node {
            Node::detach(child);
        }
        // The tree builder guarantees that the sibling has a parent.
        let parent = sibling
            .parent
            .borrow()
            .upgrade()
            .expect("sibling has a parent");
        let position = parent
            .children
            .borrow()
            .iter()
            .position(|child| Rc::ptr_eq(child, sibling))
            .expect("parent owns sibling");
        Node::insert(&parent, position, node);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attributes: Vec<Attribute>) {
        let NodeData::Element { attrs, .. } = &target.data else {
            // TreeSink guarantees that this method receives an element.
            unreachable!("HTML tree builder requested attributes on a non-element");
        };
        let mut attrs = attrs.borrow_mut();
        for attribute in attributes {
            if !attrs.iter().any(|existing| existing.name == attribute.name) {
                attrs.push(attribute);
            }
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        Node::detach(target);
    }

    fn reparent_children(&self, target: &Handle, new_parent: &Handle) {
        let children = target.children.take();
        for child in &children {
            *child.parent.borrow_mut() = Rc::downgrade(new_parent);
        }
        new_parent.children.borrow_mut().extend(children);
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(
            target.data,
            NodeData::Element {
                mathml_annotation: true,
                ..
            }
        )
    }
}

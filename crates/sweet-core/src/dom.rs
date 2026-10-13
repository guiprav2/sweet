//! A small retained DOM. Nodes are elements, text, or comments. Styling is a
//! Tailwind class string. There is no HTML parser and no `style` attribute.
//! Images are decoded pixels on an element, set from JavaScript.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);

use crate::error::SweetError;
use crate::text::FontBook;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

struct Slot {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    kind: Kind,
}

pub(crate) struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Text-editing state for an `input` or `textarea`. The window fills the
/// glyphs with egui; Tailwind still paints the box.
#[derive(Clone)]
pub(crate) struct Control {
    pub value: String,
    pub placeholder: String,
    pub password: bool,
    pub multiline: bool,
}

enum Kind {
    Element {
        tag: String,
        class_name: String,
        id_attr: String,
        attrs: HashMap<String, String>,
        image: Option<Arc<Raster>>,
        control: Option<Control>,
        disabled: bool,
        scroll_left: f32,
        scroll_top: f32,
        scroll_width: f32,
        scroll_height: f32,
        client_width: f32,
        client_height: f32,
    },
    Text {
        text: String,
    },
    Comment {
        data: String,
    },
}

struct Inner {
    nodes: Vec<Slot>,
    dark: bool,
    /// Bumps on structural, styling, and intrinsic-size changes.
    epoch: u64,
    image_epoch: u64,
}

struct Hook {
    token: u64,
    func: Arc<dyn Fn() + Send + Sync>,
}

pub struct Document {
    inner: Mutex<Inner>,
    hook: Mutex<Option<Hook>>,
    pub(crate) fonts: Arc<FontBook>,
    /// Stable id for layer and transition caches. Node ids restart per document.
    pub(crate) id: u64,
}

impl Document {
    pub fn new() -> Result<Self, SweetError> {
        let fonts = Arc::new(FontBook::load()?);
        let body = Slot {
            parent: None,
            children: Vec::new(),
            kind: element_kind("body"),
        };
        Ok(Self {
            inner: Mutex::new(Inner {
                nodes: vec![body],
                dark: false,
                epoch: 1,
                image_epoch: 1,
            }),
            hook: Mutex::new(None),
            fonts,
            id: NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn body(&self) -> NodeId {
        NodeId(0)
    }

    pub fn set_redraw_hook(&self, token: u64, func: Arc<dyn Fn() + Send + Sync>) {
        *self.hook.lock().expect("hook") = Some(Hook { token, func });
    }

    pub fn clear_redraw_hook(&self, token: u64) {
        let mut hook = self.hook.lock().expect("hook");
        if hook.as_ref().is_some_and(|hook| hook.token == token) {
            *hook = None;
        }
    }

    pub fn create_element(&self, tag: &str) -> Result<NodeId, SweetError> {
        if tag.is_empty() || tag.chars().any(|ch| ch.is_whitespace()) {
            return Err(SweetError::new("element tag must be a non-empty token"));
        }
        self.alloc(element_kind(tag))
    }

    pub fn create_comment(&self, data: &str) -> NodeId {
        self.alloc(Kind::Comment {
            data: data.to_string(),
        })
        .expect("comment")
    }

    pub fn create_text(&self, text: &str) -> NodeId {
        self.alloc(Kind::Text {
            text: text.to_string(),
        })
        .expect("text node")
    }

    pub fn append_child(&self, parent: NodeId, child: NodeId) -> Result<(), SweetError> {
        self.insert_before(parent, child, None)
    }

    pub fn insert_before(
        &self,
        parent: NodeId,
        child: NodeId,
        before: Option<NodeId>,
    ) -> Result<(), SweetError> {
        self.with_mut(|inner| {
            ensure_element(inner, parent)?;
            if child == NodeId(0) {
                return Err(SweetError::new("cannot move the document body"));
            }
            if child == parent || is_ancestor(inner, child, parent) {
                return Err(SweetError::new("cannot insert a node under itself"));
            }
            if let Some(before) = before {
                if before == child {
                    return Err(SweetError::new("insertBefore reference is the node itself"));
                }
                let index = inner.nodes[parent.0 as usize]
                    .children
                    .iter()
                    .position(|id| *id == before)
                    .ok_or_else(|| SweetError::new("insertBefore reference is not a child"))?;
                detach(inner, child);
                inner.nodes[child.0 as usize].parent = Some(parent);
                inner.nodes[parent.0 as usize].children.insert(index, child);
            } else {
                detach(inner, child);
                inner.nodes[child.0 as usize].parent = Some(parent);
                inner.nodes[parent.0 as usize].children.push(child);
            }
            Ok(())
        })
    }

    pub fn remove_child(&self, parent: NodeId, child: NodeId) -> Result<(), SweetError> {
        self.with_mut(|inner| {
            let children = &inner.nodes[parent.0 as usize].children;
            if !children.contains(&child) {
                return Err(SweetError::new("node is not a child of this parent"));
            }
            detach(inner, child);
            Ok(())
        })
    }

    pub fn set_class_name(&self, id: NodeId, class_name: &str) -> Result<(), SweetError> {
        self.with_mut(|inner| {
            match &mut slot(inner, id)?.kind {
                Kind::Element {
                    class_name: slot, ..
                } => *slot = class_name.to_string(),
                Kind::Text { .. } | Kind::Comment { .. } => {
                    return Err(SweetError::new("only elements have a class"))
                }
            }
            Ok(())
        })
    }

    pub fn class_name(&self, id: NodeId) -> Result<String, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element { class_name, .. } => Ok(class_name.clone()),
            Kind::Text { .. } | Kind::Comment { .. } => Ok(String::new()),
        }
    }

    pub fn set_text_content(&self, id: NodeId, text: &str) -> Result<(), SweetError> {
        self.with_mut(|inner| match &mut slot(inner, id)?.kind {
            Kind::Text { text: slot } => {
                *slot = text.to_string();
                Ok(())
            }
            Kind::Comment { data } => {
                *data = text.to_string();
                Ok(())
            }
            Kind::Element { .. } => {
                let children = inner.nodes[id.0 as usize].children.clone();
                for child in children {
                    detach(inner, child);
                }
                let text_id = NodeId(inner.nodes.len() as u32);
                inner.nodes.push(Slot {
                    parent: Some(id),
                    children: Vec::new(),
                    kind: Kind::Text {
                        text: text.to_string(),
                    },
                });
                inner.nodes[id.0 as usize].children.push(text_id);
                Ok(())
            }
        })
    }

    pub fn text_content(&self, id: NodeId) -> Result<String, SweetError> {
        let inner = self.inner.lock().expect("dom");
        fn walk(inner: &Inner, id: NodeId, out: &mut String) {
            match &inner.nodes[id.0 as usize].kind {
                Kind::Text { text } => out.push_str(text),
                Kind::Comment { data } => out.push_str(data),
                Kind::Element { .. } => {
                    for child in &inner.nodes[id.0 as usize].children {
                        if matches!(inner.nodes[child.0 as usize].kind, Kind::Comment { .. }) {
                            continue;
                        }
                        walk(inner, *child, out);
                    }
                }
            }
        }
        let _ = slot_ref(&inner, id)?;
        let mut out = String::new();
        walk(&inner, id, &mut out);
        Ok(out)
    }

    pub fn set_id_attr(&self, id: NodeId, value: &str) -> Result<(), SweetError> {
        self.with_mut(|inner| {
            match &mut slot(inner, id)?.kind {
                Kind::Element { id_attr, .. } => *id_attr = value.to_string(),
                Kind::Text { .. } | Kind::Comment { .. } => {
                    return Err(SweetError::new("only elements have an id attribute"))
                }
            }
            Ok(())
        })
    }

    pub fn id_attr(&self, id: NodeId) -> Result<String, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element { id_attr, .. } => Ok(id_attr.clone()),
            Kind::Text { .. } | Kind::Comment { .. } => Ok(String::new()),
        }
    }

    pub fn tag_name(&self, id: NodeId) -> Result<String, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element { tag, .. } => Ok(tag.clone()),
            Kind::Text { .. } => Ok("#text".into()),
            Kind::Comment { .. } => Ok("#comment".into()),
        }
    }

    /// 1 element, 3 text, 8 comment. Same numbers as the DOM `nodeType` constants.
    pub fn node_type(&self, id: NodeId) -> Result<u32, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element { .. } => Ok(1),
            Kind::Text { .. } => Ok(3),
            Kind::Comment { .. } => Ok(8),
        }
    }

    pub fn set_attribute(&self, id: NodeId, name: &str, value: &str) -> Result<(), SweetError> {
        if name.eq_ignore_ascii_case("style") {
            return Err(SweetError::new(
                "sweet has no CSS; set className to Tailwind utilities",
            ));
        }
        match name {
            "class" => self.set_class_name(id, value),
            "id" => self.set_id_attr(id, value),
            "value" | "placeholder" | "type" | "disabled" => {
                self.with_mut(|inner| match &mut slot(inner, id)?.kind {
                    Kind::Element {
                        attrs,
                        control,
                        disabled,
                        ..
                    } => {
                        if name == "disabled" {
                            *disabled = value != "false";
                        }
                        attrs.insert(name.to_string(), value.to_string());
                        if let Some(control) = control {
                            match name {
                                "value" => control.value = value.to_string(),
                                "placeholder" => control.placeholder = value.to_string(),
                                "type" => control.password = value == "password",
                                _ => {}
                            }
                        }
                        Ok(())
                    }
                    Kind::Text { .. } | Kind::Comment { .. } => {
                        Err(SweetError::new("only elements have attributes"))
                    }
                })
            }
            _ => self.with_mut(|inner| match &mut slot(inner, id)?.kind {
                Kind::Element { attrs, .. } => {
                    attrs.insert(name.to_string(), value.to_string());
                    Ok(())
                }
                Kind::Text { .. } | Kind::Comment { .. } => {
                    Err(SweetError::new("only elements have attributes"))
                }
            }),
        }
    }

    pub fn attribute(&self, id: NodeId, name: &str) -> Result<Option<String>, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element {
                class_name,
                id_attr,
                attrs,
                ..
            } => Ok(match name {
                "class" => Some(class_name.clone()),
                "id" => Some(id_attr.clone()),
                other => attrs.get(other).cloned(),
            }),
            Kind::Text { .. } | Kind::Comment { .. } => Ok(None),
        }
    }

    pub fn remove_attribute(&self, id: NodeId, name: &str) -> Result<(), SweetError> {
        if name.eq_ignore_ascii_case("style") {
            return Ok(());
        }
        match name {
            "class" => self.set_class_name(id, ""),
            "id" => self.set_id_attr(id, ""),
            _ => self.with_mut(|inner| match &mut slot(inner, id)?.kind {
                Kind::Element { attrs, .. } => {
                    attrs.remove(name);
                    Ok(())
                }
                Kind::Text { .. } | Kind::Comment { .. } => {
                    Err(SweetError::new("only elements have attributes"))
                }
            }),
        }
    }

    pub fn set_image_rgba(
        &self,
        id: NodeId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), SweetError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if width == 0 || height == 0 || rgba.len() != expected {
            return Err(SweetError::new(
                "image pixels do not match the width and height",
            ));
        }
        self.store_image(id, width, height, rgba)
    }

    pub fn set_image_encoded(&self, id: NodeId, bytes: &[u8]) -> Result<(u32, u32), SweetError> {
        let decoded = image::load_from_memory(bytes)
            .map_err(|err| SweetError::new(format!("could not decode image: {err}")))?;
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.store_image(id, width, height, rgba.into_raw())?;
        Ok((width, height))
    }

    pub fn control_value(&self, id: NodeId) -> Result<String, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element {
                control: Some(control),
                ..
            } => Ok(control.value.clone()),
            _ => Ok(String::new()),
        }
    }

    pub fn set_control_value(&self, id: NodeId, value: &str) -> Result<(), SweetError> {
        self.with_mut(|inner| match &mut slot(inner, id)?.kind {
            Kind::Element {
                attrs,
                control: Some(control),
                ..
            } => {
                control.value = value.to_string();
                attrs.insert("value".into(), value.to_string());
                Ok(())
            }
            _ => Err(SweetError::new("only input and textarea have a value")),
        })
    }

    pub fn is_disabled(&self, id: NodeId) -> Result<bool, SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element { disabled, .. } => Ok(*disabled),
            _ => Ok(false),
        }
    }

    pub fn set_disabled(&self, id: NodeId, value: bool) -> Result<(), SweetError> {
        self.with_mut(|inner| match &mut slot(inner, id)?.kind {
            Kind::Element { disabled, .. } => {
                *disabled = value;
                Ok(())
            }
            _ => Ok(()),
        })
    }

    pub fn clear_image(&self, id: NodeId) -> Result<(), SweetError> {
        self.with_mut(|inner| match &mut slot(inner, id)?.kind {
            Kind::Element { image, .. } => {
                *image = None;
                Ok(())
            }
            Kind::Text { .. } | Kind::Comment { .. } => {
                Err(SweetError::new("only elements can hold an image"))
            }
        })
    }

    fn store_image(
        &self,
        id: NodeId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), SweetError> {
        // Video replaces the same pixels many times a second. Skip the redraw
        // hook when the picture is unchanged, and never hold the DOM lock
        // across the notify — the window thread paints under that lock.
        let changed = {
            let mut inner = self.inner.lock().expect("dom");
            match &mut slot(&mut inner, id)?.kind {
                Kind::Element { image, .. } => {
                    if image.as_ref().is_some_and(|current| {
                        current.width == width && current.height == height && current.rgba == rgba
                    }) {
                        false
                    } else {
                        let same_size = image.as_ref().is_some_and(|current| {
                            current.width == width && current.height == height
                        });
                        *image = Some(Arc::new(Raster {
                            width,
                            height,
                            rgba,
                        }));
                        if !same_size {
                            inner.epoch = inner.epoch.wrapping_add(1);
                        }
                        inner.image_epoch = inner.image_epoch.wrapping_add(1);
                        true
                    }
                }
                Kind::Text { .. } | Kind::Comment { .. } => {
                    return Err(SweetError::new("only elements can hold an image"));
                }
            }
        };
        if changed {
            self.notify();
        }
        Ok(())
    }

    pub fn children(&self, id: NodeId) -> Result<Vec<NodeId>, SweetError> {
        let inner = self.inner.lock().expect("dom");
        Ok(slot_ref(&inner, id)?.children.clone())
    }

    pub fn parent(&self, id: NodeId) -> Result<Option<NodeId>, SweetError> {
        let inner = self.inner.lock().expect("dom");
        Ok(slot_ref(&inner, id)?.parent)
    }

    pub fn scroll_offset(&self, id: NodeId) -> Result<(f32, f32), SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element {
                scroll_left,
                scroll_top,
                ..
            } => Ok((*scroll_left, *scroll_top)),
            Kind::Text { .. } | Kind::Comment { .. } => {
                Err(SweetError::new("only elements can scroll"))
            }
        }
    }

    pub fn set_scroll_offset(&self, id: NodeId, left: f32, top: f32) -> Result<(), SweetError> {
        self.with_mut(|inner| match &mut slot(inner, id)?.kind {
            Kind::Element {
                scroll_left,
                scroll_top,
                ..
            } => {
                *scroll_left = if left.is_finite() { left.max(0.0) } else { 0.0 };
                *scroll_top = if top.is_finite() { top.max(0.0) } else { 0.0 };
                Ok(())
            }
            Kind::Text { .. } | Kind::Comment { .. } => {
                Err(SweetError::new("only elements can scroll"))
            }
        })
    }

    pub fn scroll_metrics(&self, id: NodeId) -> Result<(f32, f32, f32, f32), SweetError> {
        let inner = self.inner.lock().expect("dom");
        match &slot_ref(&inner, id)?.kind {
            Kind::Element {
                scroll_width,
                scroll_height,
                client_width,
                client_height,
                ..
            } => Ok((*scroll_width, *scroll_height, *client_width, *client_height)),
            Kind::Text { .. } | Kind::Comment { .. } => {
                Err(SweetError::new("only elements have scroll metrics"))
            }
        }
    }

    pub(crate) fn update_scroll_metrics(
        &self,
        id: NodeId,
        scroll_width: f32,
        scroll_height: f32,
        client_width: f32,
        client_height: f32,
    ) {
        let mut inner = self.inner.lock().expect("dom");
        if let Some(Slot {
            kind:
                Kind::Element {
                    scroll_left,
                    scroll_top,
                    scroll_width: sw,
                    scroll_height: sh,
                    client_width: cw,
                    client_height: ch,
                    ..
                },
            ..
        }) = inner.nodes.get_mut(id.0 as usize)
        {
            *sw = scroll_width.max(0.0);
            *sh = scroll_height.max(0.0);
            *cw = client_width.max(0.0);
            *ch = client_height.max(0.0);
            *scroll_left = scroll_left.min((*sw - *cw).max(0.0));
            *scroll_top = scroll_top.min((*sh - *ch).max(0.0));
        }
    }

    pub fn set_dark(&self, dark: bool) {
        self.with_mut(|inner| inner.dark = dark);
    }

    pub fn dark(&self) -> bool {
        self.inner.lock().expect("dom").dark
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let inner = self.inner.lock().expect("dom");
        let nodes = inner
            .nodes
            .iter()
            .enumerate()
            .map(|(index, slot)| SnapNode {
                id: NodeId(index as u32),
                parent: slot.parent,
                children: slot.children.clone(),
                kind: match &slot.kind {
                    Kind::Element {
                        class_name,
                        image,
                        control,
                        disabled,
                        scroll_left,
                        scroll_top,
                        ..
                    } => SnapKind::Element {
                        class_name: class_name.clone(),
                        image: image.clone(),
                        control: control.clone(),
                        disabled: *disabled,
                        scroll_left: *scroll_left,
                        scroll_top: *scroll_top,
                    },
                    Kind::Text { text } => SnapKind::Text { text: text.clone() },
                    Kind::Comment { data } => SnapKind::Comment { data: data.clone() },
                },
            })
            .collect();
        Snapshot {
            nodes,
            body: NodeId(0),
            dark: inner.dark,
            image_epoch: inner.image_epoch,
        }
    }

    /// Refresh pixels only when every changed image belongs to a replayed layer.
    /// Static pixels and intrinsic-size changes require a full scene rebuild.
    pub(crate) fn refresh_layer_images(
        &self,
        snapshot: &mut Snapshot,
        epoch: u64,
        replayed: &[bool],
    ) -> bool {
        let inner = self.inner.lock().expect("dom");
        if inner.epoch != epoch {
            return false;
        }
        if inner.image_epoch == snapshot.image_epoch {
            return true;
        }
        for (index, node) in snapshot.nodes.iter_mut().enumerate() {
            if let (SnapKind::Element { image: previous, .. }, Kind::Element { image: current, .. }) =
                (&mut node.kind, &inner.nodes[index].kind)
            {
                let unchanged = match (previous.as_ref(), current.as_ref()) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                if !unchanged {
                    if !replayed[index] {
                        return false;
                    }
                    *previous = current.clone();
                }
            }
        }
        snapshot.image_epoch = inner.image_epoch;
        true
    }

    fn alloc(&self, kind: Kind) -> Result<NodeId, SweetError> {
        self.with_mut(|inner| {
            let id = NodeId(inner.nodes.len() as u32);
            inner.nodes.push(Slot {
                parent: None,
                children: Vec::new(),
                kind,
            });
            Ok(id)
        })
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.inner.lock().expect("dom").epoch
    }

    fn with_mut<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let value = {
            let mut inner = self.inner.lock().expect("dom");
            inner.epoch = inner.epoch.wrapping_add(1);
            f(&mut inner)
        };
        self.notify();
        value
    }

    fn notify(&self) {
        let func = self
            .hook
            .lock()
            .expect("hook")
            .as_ref()
            .map(|hook| Arc::clone(&hook.func));
        if let Some(func) = func {
            func();
        }
    }
}

fn slot(inner: &mut Inner, id: NodeId) -> Result<&mut Slot, SweetError> {
    inner
        .nodes
        .get_mut(id.0 as usize)
        .ok_or_else(|| SweetError::new("unknown node"))
}

fn slot_ref(inner: &Inner, id: NodeId) -> Result<&Slot, SweetError> {
    inner
        .nodes
        .get(id.0 as usize)
        .ok_or_else(|| SweetError::new("unknown node"))
}

fn element_kind(tag: &str) -> Kind {
    let control = match tag {
        "input" => Some(Control {
            value: String::new(),
            placeholder: String::new(),
            password: false,
            multiline: false,
        }),
        "textarea" => Some(Control {
            value: String::new(),
            placeholder: String::new(),
            password: false,
            multiline: true,
        }),
        _ => None,
    };
    Kind::Element {
        tag: tag.to_string(),
        class_name: String::new(),
        id_attr: String::new(),
        attrs: HashMap::new(),
        image: None,
        control,
        disabled: false,
        scroll_left: 0.0,
        scroll_top: 0.0,
        scroll_width: 0.0,
        scroll_height: 0.0,
        client_width: 0.0,
        client_height: 0.0,
    }
}

fn ensure_element(inner: &Inner, id: NodeId) -> Result<(), SweetError> {
    match &slot_ref(inner, id)?.kind {
        Kind::Element { .. } => Ok(()),
        Kind::Text { .. } => Err(SweetError::new("text nodes cannot have children")),
        Kind::Comment { .. } => Err(SweetError::new("comments cannot have children")),
    }
}

fn is_ancestor(inner: &Inner, ancestor: NodeId, node: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = inner.nodes[id.0 as usize].parent;
    }
    false
}

fn detach(inner: &mut Inner, child: NodeId) {
    if let Some(parent) = inner.nodes[child.0 as usize].parent.take() {
        inner.nodes[parent.0 as usize]
            .children
            .retain(|id| *id != child);
    }
}

pub(crate) struct Snapshot {
    image_epoch: u64,
    pub nodes: Vec<SnapNode>,
    pub body: NodeId,
    pub dark: bool,
}

pub(crate) struct SnapNode {
    #[allow(dead_code)]
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub kind: SnapKind,
}

pub(crate) enum SnapKind {
    Element {
        class_name: String,
        image: Option<Arc<Raster>>,
        control: Option<Control>,
        disabled: bool,
        scroll_left: f32,
        scroll_top: f32,
    },
    Text {
        text: String,
    },
    Comment {
        #[allow(dead_code)]
        data: String,
    },
}

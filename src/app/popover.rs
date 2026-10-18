//! A trigger with a menu floating under it, for the settings dropdowns. The menu draws over
//! everything, flips above the trigger when there is no room below, and a press anywhere else
//! dismisses it.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector};

pub struct Popover<'a, Message> {
    anchor: Element<'a, Message>,
    menu: Element<'a, Message>,
    open: bool,
    on_dismiss: Message,
    /// menu width; the trigger's width when unset
    width: Option<f32>,
}

const GAP: f32 = 6.0;
const EDGE: f32 = 8.0;

impl<'a, Message: Clone + 'a> Popover<'a, Message> {
    pub fn new(anchor: impl Into<Element<'a, Message>>, menu: impl Into<Element<'a, Message>>, open: bool, on_dismiss: Message) -> Self {
        Self { anchor: anchor.into(), menu: menu.into(), open, on_dismiss, width: None }
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, Theme, Renderer> for Popover<'a, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.anchor), Tree::new(&self.menu)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.anchor.as_widget(), self.menu.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.anchor.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.anchor.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn widget::Operation) {
        self.anchor.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer, clipboard: &mut dyn Clipboard, shell: &mut Shell<'_, Message>, viewport: &Rectangle) {
        self.anchor.as_widget_mut().update(&mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle, renderer: &Renderer) -> mouse::Interaction {
        self.anchor.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(&self, tree: &Tree, renderer: &mut Renderer, theme: &Theme, style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle) {
        self.anchor.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &Renderer, viewport: &Rectangle, translation: Vector) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let mut children = tree.children.iter_mut();
        let anchor_tree = children.next().unwrap();
        if !self.open {
            return self.anchor.as_widget_mut().overlay(anchor_tree, layout, renderer, viewport, translation);
        }
        let bounds = layout.bounds() + translation;
        Some(overlay::Element::new(Box::new(Menu { menu: &mut self.menu, tree: children.next().unwrap(), anchor: bounds, width: self.width.unwrap_or(bounds.width), on_dismiss: self.on_dismiss.clone() })))
    }
}

struct Menu<'a, 'b, Message> {
    menu: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    anchor: Rectangle,
    width: f32,
    on_dismiss: Message,
}

impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Menu<'_, '_, Message> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let below = bounds.height - (self.anchor.y + self.anchor.height + GAP) - EDGE;
        let above = self.anchor.y - GAP - EDGE;
        let room = below.max(above).max(80.0);
        let node = self.menu.as_widget_mut().layout(self.tree, renderer, &layout::Limits::new(Size::new(self.width, 0.0), Size::new(self.width, room)));
        let h = node.size().height;
        let y = if h <= below || below >= above { self.anchor.y + self.anchor.height + GAP } else { self.anchor.y - GAP - h };
        // right-align with the trigger when the menu is wider, and stay on screen
        let x = (self.anchor.x + self.anchor.width - self.width).min(self.anchor.x).clamp(EDGE, (bounds.width - self.width - EDGE).max(EDGE));
        node.move_to(Point::new(x, y))
    }

    fn draw(&self, renderer: &mut Renderer, theme: &Theme, style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor) {
        self.menu.as_widget().draw(self.tree, renderer, theme, style, layout, cursor, &layout.bounds());
    }

    fn update(&mut self, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer, clipboard: &mut dyn Clipboard, shell: &mut Shell<'_, Message>) {
        let b = layout.bounds();
        self.menu.as_widget_mut().update(self.tree, event, layout, cursor, renderer, clipboard, shell, &b);
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event {
            // the trigger toggles itself; anywhere else closes the menu
            if !cursor.is_over(b) && !cursor.is_over(self.anchor) {
                shell.publish(self.on_dismiss.clone());
            }
        }
        if cursor.is_over(b) && matches!(event, Event::Mouse(_)) {
            shell.capture_event();
        }
    }

    fn mouse_interaction(&self, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        self.menu.as_widget().mouse_interaction(self.tree, layout, cursor, &layout.bounds(), renderer)
    }
}

impl<'a, Message: Clone + 'a> From<Popover<'a, Message>> for Element<'a, Message> {
    fn from(p: Popover<'a, Message>) -> Self {
        Element::new(p)
    }
}

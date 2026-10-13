//! GPUI paint lifetime, clipping and foreground-overlay fallback for compositor elements.
//! This module has no OS types or widget timing; the backend consumes generic layer models.

use std::{
	cell::{Cell, RefCell},
	collections::HashMap,
	rc::{Rc, Weak},
};

use gpui_kit::{
	AnyElement, App, Bounds, Element, ElementId, Global, GlobalElementId, InspectorElementId, IntoElement as _, LayoutId, Pixels,
	SharedString, Subscription, Window, WindowId, px,
};

use super::{
	macos::{Canvas, Parent},
	model::Kind,
};

#[derive(Default)]
struct Meta {
	occluded: Cell<bool>,
	painted:  RefCell<Weak<RefCell<Widget>>>,
	runtime:  RefCell<Weak<Runtime>>,
}

/// Layout owns only metadata and a weak paint link. Cached paint keeps the strong widget
/// alive; omitting paint (hidden, clipped out or removed) drops the native canvas.
#[derive(Clone, Default)]
pub struct Link(Rc<Meta>);

impl Link {
	pub fn is_native(&self) -> bool {
		self.0.painted.borrow().upgrade().is_some_and(|widget| widget.borrow().canvas.is_some())
	}

	pub fn set_text(&self, label: &SharedString) {
		if let Some(widget) = self.0.painted.borrow().upgrade()
			&& let Some(canvas) = &mut widget.borrow_mut().canvas
		{
			canvas.set_text(label);
		}
	}
}

struct Widget {
	bounds: Bounds<Pixels>,
	canvas: Option<Canvas>,
}

struct Painted {
	widget:   Rc<RefCell<Widget>>,
	_runtime: Rc<Runtime>,
}

#[derive(Default)]
struct WindowState {
	obscured: bool,
	runtime:  Weak<Runtime>,
}

#[derive(Default)]
struct Registry(HashMap<WindowId, WindowState>);
impl Global for Registry {}

struct Runtime {
	links:         RefCell<Vec<Weak<Meta>>>,
	_subscription: Subscription,
}

fn registry(cx: &mut App) -> &mut Registry {
	if !cx.has_global::<Registry>() {
		cx.set_global(Registry::default());
	}
	cx.global_mut::<Registry>()
}

pub fn set_obscured(window: &mut Window, cx: &mut App, obscured: bool) {
	let id = window.window_handle().window_id();
	let registry = registry(cx);
	registry.0.retain(|other, state| *other == id || state.runtime.strong_count() > 0);
	registry.0.entry(id).or_default().obscured = obscured;
}

fn runtime(window: &mut Window, cx: &mut App) -> Rc<Runtime> {
	let id = window.window_handle().window_id();
	if let Some(runtime) = registry(cx).0.entry(id).or_default().runtime.upgrade() {
		return runtime;
	}
	let runtime = Rc::new_cyclic(|weak: &Weak<Runtime>| {
		let weak = weak.clone();
		let subscription = window.observe_native_element_paint(move |window, cx| {
			let Some(runtime) = weak.upgrade() else {
				return;
			};
			let occlusion = window.native_element_paint_context(cx).occluding_bounds;
			let obscured = registry(cx).0.get(&window.window_handle().window_id()).is_some_and(|state| state.obscured);
			let mut changed = false;
			runtime.links.borrow_mut().retain(|weak| {
				let Some(meta) = weak.upgrade() else {
					return false;
				};
				if let Some(widget) = meta.painted.borrow().upgrade() {
					let bounds = widget.borrow().bounds;
					let occluded = obscured || occlusion.is_some_and(|overlay| intersects(bounds, overlay));
					changed |= meta.occluded.replace(occluded) != occluded;
				}
				true
			});
			if changed {
				// GPUI rerenders cached content once, before presenting this same scene.
				window.refresh();
			}
		});
		Runtime { links: RefCell::new(Vec::new()), _subscription: subscription }
	});
	registry(cx).0.entry(id).or_default().runtime = Rc::downgrade(&runtime);
	runtime
}

fn intersects(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
	let overlap = a.intersect(&b);
	overlap.size.width > px(0.) && overlap.size.height > px(0.)
}

pub fn element(id: &'static str, kind: Kind, link: Option<Link>) -> AnyElement {
	NativeElement { id, kind, link }.into_any_element()
}

struct NativeElement {
	id:   &'static str,
	kind: Kind,
	link: Option<Link>,
}

struct Layout {
	element:    AnyElement,
	parent:     Option<Parent>,
	composited: bool,
	link:       Link,
}

impl Element for NativeElement {
	type RequestLayoutState = Layout;
	type PrepaintState = ();

	fn id(&self) -> Option<ElementId> {
		Some(self.id.into())
	}

	fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
		None
	}

	fn request_layout(
		&mut self,
		global_id: Option<&GlobalElementId>,
		_: Option<&InspectorElementId>,
		window: &mut Window,
		cx: &mut App,
	) -> (LayoutId, Layout) {
		let link = if let Some(id) = global_id {
			window.with_element_state(id, |old: Option<Link>, _| {
				let link = old.unwrap_or_else(|| self.link.clone().unwrap_or_default());
				(link.clone(), link)
			})
		} else {
			self.link.clone().unwrap_or_default()
		};
		if let Some(widget) = link.0.painted.borrow().upgrade()
			&& let Some(canvas) = &widget.borrow().canvas
		{
			canvas.hide();
		}
		let parent = Parent::for_window(window);
		let obscured = registry(cx).0.get(&window.window_handle().window_id()).is_some_and(|state| state.obscured);
		let composited = parent.is_some() && !self.kind.needs_gpui() && !link.0.occluded.get() && !obscured;
		let mut element = if composited { self.kind.placeholder() } else { self.kind.fallback(self.id) };
		let layout = element.request_layout(window, cx);
		(layout, Layout { element, parent, composited, link })
	}

	fn prepaint(
		&mut self,
		_: Option<&GlobalElementId>,
		_: Option<&InspectorElementId>,
		_: Bounds<Pixels>,
		state: &mut Layout,
		window: &mut Window,
		cx: &mut App,
	) {
		state.element.prepaint(window, cx);
	}

	fn paint(
		&mut self,
		global_id: Option<&GlobalElementId>,
		_: Option<&InspectorElementId>,
		bounds: Bounds<Pixels>,
		state: &mut Layout,
		_: &mut (),
		window: &mut Window,
		cx: &mut App,
	) {
		let (Some(parent), Some(id)) = (&state.parent, global_id) else {
			state.element.paint(window, cx);
			return;
		};
		let paint_bounds = self.kind.paint_bounds(bounds);
		let clip = paint_bounds.intersect(&window.content_mask().bounds);
		if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
			return;
		}
		if !state.composited {
			state.element.paint(window, cx);
		}
		let runtime = runtime(window, cx);
		let context = window.native_element_paint_context(cx);
		let scale = f64::from(window.scale_factor());
		let layers = state
			.composited
			.then(|| self.kind.layers(bounds, window.text_style().color.to_rgb(), cx.reduce_motion(), context.animation_epoch));
		window.with_element_state(id, |old: Option<Painted>, _| {
			let painted = old.unwrap_or_else(|| {
				if state.link.0.runtime.borrow().as_ptr() != Rc::as_ptr(&runtime) {
					runtime.links.borrow_mut().push(Rc::downgrade(&state.link.0));
					*state.link.0.runtime.borrow_mut() = Rc::downgrade(&runtime);
				}
				Painted { widget: Rc::new(RefCell::new(Widget { bounds: paint_bounds, canvas: None })), _runtime: runtime }
			});
			*state.link.0.painted.borrow_mut() = Rc::downgrade(&painted.widget);
			let mut widget = painted.widget.borrow_mut();
			widget.bounds = paint_bounds;
			if let Some(layers) = layers {
				let canvas = widget.canvas.get_or_insert_with(|| Canvas::new(parent));
				canvas.paint(bounds, clip, scale, context.opacity, layers);
			} else {
				widget.canvas = None;
			}
			drop(widget);
			((), painted)
		});
	}
}

impl gpui_kit::IntoElement for NativeElement {
	type Element = Self;
	fn into_element(self) -> Self {
		self
	}
}

//! GPUI quiescent reclamation and compact scene replay without a native window or credentials.

#![expect(
	unused_crate_dependencies,
	reason = "the GPUI integration test exercises the public UI API, not the app's implementation dependencies"
)]

#[cfg(test)]
mod tests {
	use std::{cell::Cell, rc::Rc};

	use gpui_kit::{FocusHandle, Keystroke, Window, WindowHandle, WindowVisibility, div, prelude::*, px, rgb};
	use gpui_test::TestAppContext;

	struct IdleView {
		renders: Rc<Cell<usize>>,
		keys:    Rc<Cell<usize>>,
		focus:   FocusHandle,
	}

	impl Render for IdleView {
		fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
			self.renders.set(self.renders.get() + 1);
			let keys = self.keys.clone();
			div()
				.id("idle-root")
				.track_focus(&self.focus)
				.size_full()
				.bg(rgb(0x0010_2030))
				.on_key_down(move |_, _, _| keys.set(keys.get() + 1))
				.child(
					div()
						.id("clipped-layer")
						.w(px(80.25))
						.h(px(60.5))
						.overflow_hidden()
						.bg(rgb(0x0040_5060))
						.child(div().w(px(100.5)).h(px(90.25)).bg(rgb(0x0070_8090))),
				)
		}
	}

	fn draw(window: WindowHandle<IdleView>, cx: &mut TestAppContext, refresh: bool) -> String {
		cx.update_window(window.into(), |_, window, cx| {
			if refresh {
				window.refresh();
			}
			window.draw(cx).clear(cx);
			let quads = window.painted_quads();
			assert!(!quads.is_empty());
			format!("{quads:?}")
		})
		.unwrap()
	}

	#[test]
	fn idle_and_hidden_preserve_live_paint_focus_and_input() {
		let mut app = TestAppContext::single();
		let cx = &mut app;
		for hidden in [false, true] {
			let renders = Rc::new(Cell::new(0));
			let keys = Rc::new(Cell::new(0));
			let window = cx.add_window({
				let renders = renders.clone();
				let keys = keys.clone();
				move |_, cx| IdleView { renders, keys, focus: cx.focus_handle() }
			});
			window.update(cx, |view, window, cx| window.focus(&view.focus, cx)).unwrap();
			draw(window, cx, true);
			let before = draw(window, cx, false);
			cx.dispatch_keystroke(window.into(), Keystroke::parse("a").unwrap());
			assert_eq!(keys.get(), 1);
			let renders_before = renders.get();

			if hidden {
				cx.simulate_window_visibility_change(window.into(), WindowVisibility::Hidden);
			} else {
				// Repeated notifications must also be harmless and must not draw.
				for _ in 0..3 {
					cx.simulate_window_frame_idle(window.into());
				}
			}
			assert_eq!(renders.get(), renders_before);
			window
				.update(cx, |view, window, _| {
					assert!(view.focus.is_focused(window));
					assert_eq!(format!("{:?}", window.painted_quads()), before);
				})
				.unwrap();

			if hidden {
				cx.simulate_window_visibility_change(window.into(), WindowVisibility::Visible);
			}
			cx.dispatch_keystroke(window.into(), Keystroke::parse("b").unwrap());
			assert_eq!(keys.get(), 2);
			assert_eq!(draw(window, cx, false), before, "cached replay after idle");
			assert_eq!(draw(window, cx, true), before, "full layout after idle");
			cx.dispatch_keystroke(window.into(), Keystroke::parse("c").unwrap());
			assert_eq!(keys.get(), 3);
		}
	}

	mod scene_replay {
		use gpui_test::{
			AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, MonochromeSprite, Path, Pixels, Primitive, Quad,
			ScaledPixels, Scene, SubpixelSprite, TileId, point, px, rgb, size,
		};

		fn bounds(x: f32, width: f32) -> Bounds<ScaledPixels> {
			Bounds::new(point(ScaledPixels(x), ScaledPixels(0.)), size(ScaledPixels(width), ScaledPixels(10.)))
		}

		fn signature(scene: &Scene) -> String {
			format!(
				"{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
				scene.shadows,
				scene.quads,
				scene.paths,
				scene.underlines,
				scene.monochrome_sprites,
				scene.subpixel_sprites,
				scene.polychrome_sprites
			)
		}

		fn sprite(index: u32, x: f32, tile_id: u32) -> MonochromeSprite {
			MonochromeSprite {
				order:          0,
				pad:            0,
				bounds:         bounds(x, 12.),
				content_mask:   ContentMask { bounds: bounds(0., 200.) },
				color:          rgb(index).into(),
				tile:           AtlasTile {
					texture_id: AtlasTextureId { index: 0, kind: AtlasTextureKind::Monochrome },
					tile_id:    TileId(tile_id),
					padding:    0,
					bounds:     Bounds::default(),
				},
				transformation: Default::default(),
			}
		}

		#[test]
		fn sorted_batches_replay_original_records_and_partial_ranges() {
			let mut random = 7_u32;
			for _ in 0..32 {
				let mut records = Vec::<Primitive>::new();
				for index in 0..48 {
					random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
					let x = (random % 100) as f32;
					let mono = sprite(index, x, random % 7);
					records.push(
						Quad { bounds: mono.bounds, content_mask: mono.content_mask, background: rgb(index).into(), ..Default::default() }
							.into(),
					);
					records.push(mono.into());
					records.push(
						SubpixelSprite {
							order:          0,
							pad:            0,
							bounds:         mono.bounds,
							content_mask:   mono.content_mask,
							color:          mono.color,
							tile:           mono.tile,
							transformation: mono.transformation,
						}
						.into(),
					);
					let mut path = Path::<Pixels>::new(point(px(x), px(0.)));
					path.line_to(point(px(x + 4.), px(8.)));
					path.line_to(point(px(x + 8.), px(0.)));
					path.content_mask = ContentMask { bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(10.))) };
					records.push(path.scale(1.).into());
				}
				let mut scene = Scene::default();
				for record in &records {
					scene.insert_primitive(record.clone());
				}
				assert_eq!(scene.len(), records.len());
				let mut quads = scene.quads.clone();
				quads.sort_by_key(|quad| quad.order);
				let mut sprites = scene.monochrome_sprites.clone();
				sprites.sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
				scene.finish();
				assert_eq!(format!("{:?}", scene.quads), format!("{quads:?}"));
				assert_eq!(format!("{:?}", scene.monochrome_sprites), format!("{sprites:?}"));
				let first_finish = signature(&scene);
				scene.finish();
				assert_eq!(signature(&scene), first_finish, "finish is repeatable");
				let mut replay = Scene::default();
				replay.replay(0..scene.len(), &scene);
				replay.finish();
				assert_eq!(signature(&replay), first_finish);
				for range in [0..1, 5..19, 70..99, 100..scene.len()] {
					let mut expected = Scene::default();
					for record in &records[range.clone()] {
						expected.insert_primitive(record.clone());
					}
					expected.finish();
					let mut partial = Scene::default();
					partial.replay(range, &scene);
					partial.finish();
					assert_eq!(signature(&partial), signature(&expected));
				}
			}
		}

		#[test]
		fn nested_layers_keep_replay_order_after_texture_sorting() {
			let mut scene = Scene::default();
			scene.push_layer(bounds(0., 200.));
			for tile in [5, 1, 5, 3, 2] {
				scene.insert_primitive(sprite(tile, tile as f32, tile));
			}
			scene.push_layer(bounds(3., 40.));
			for tile in [3, 1, 2] {
				scene.insert_primitive(sprite(tile + 10, tile as f32, tile));
			}
			scene.pop_layer();
			scene.insert_primitive(sprite(20, 8., 0));
			scene.pop_layer();
			scene.finish();
			let mut replay = Scene::default();
			replay.replay(0..scene.len(), &scene);
			replay.finish();
			assert_eq!(replay.len(), scene.len());
			assert_eq!(signature(&replay), signature(&scene));
		}
	}
}

//! First run: everything needed to go from nothing to playing, in order.

use tern_sdk::ui::{self, Node, html};

use super::{Tap, div, icon, text};
use crate::{
	app::{Field, Msg, State},
	library::Conn,
	spotify::REDIRECT_URI,
	ui::icons::{Glyph, Icons},
};

#[derive(Clone, Copy, PartialEq)]
enum Step {
	Done,
	Current,
	Todo,
}

pub fn setup(state: &State, icons: &Icons) -> Node<Msg> {
	let conn = &state.library.conn;
	let signed_in = state.signed_in;
	let app_done = !matches!(conn, Conn::NeedsClientId);
	let account_done = matches!(conn, Conn::Ready);
	let status = |done: bool, ready: bool| match (done, ready) {
		(true, _) => Step::Done,
		(false, true) => Step::Current,
		(false, false) => Step::Todo,
	};

	let mut steps = div("tt-setup").key("setup").child(text("tt-setup-title", "Set up terntable")).child(text(
		"tt-setup-lead tt-dim",
		"Three steps, once. terntable plays Spotify itself and reads your library through Spotify's Web API \
		 with an app that belongs to you. A Premium account is required.",
	));

	// 1. The player's sign-in
	let mut body = vec![text("tt-step-text tt-dim", "Sign in with Spotify in your browser so terntable can play.")];
	if state.starting_player() {
		let line = state.notice.clone().unwrap_or_else(|| "Starting the player…".to_owned());
		body.push(div("tt-step-status").child(html::span("").class("tt-dot")).child(html::span(line)).into());
	} else if !signed_in {
		if let Some(notice) = &state.notice {
			body.push(text("tt-step-text", notice.clone()));
		}
		body.push(pill("Sign in", Msg::SignIn));
	}
	steps = steps.child(step(icons, 1, "Sign in to play", status(signed_in, true), body));

	// 2. Web API app
	let mut body = vec![
		html::div()
			.class("tt-step-text tt-dim")
			.child(html::span("Create an app in the Spotify developer dashboard. Set the redirect URI to "))
			.child(html::code(REDIRECT_URI))
			.child(html::span(", tick “Web API”, save, then paste the app's Client ID."))
			.into(),
	];
	body.push(link("Open the developer dashboard", "https://developer.spotify.com/dashboard"));
	if !app_done {
		body.push(field(state, Field::ClientId, &state.id_input, "Client ID", Msg::SaveClientId(state.id_input.clone())));
	}
	steps = steps.child(step(icons, 2, "Create your Spotify app", status(app_done, signed_in), body));

	// 3. Connect the library
	let mut body =
		vec![text("tt-step-text tt-dim", "Sign in with your app in the browser so terntable can show your playlists and liked songs.")];
	match conn {
		Conn::LoggingIn(url) => {
			body.push(text("tt-step-text", "Waiting for you to finish in the browser…"));
			if let Some(url) = url {
				body.push(link("No browser? Open the sign-in page", url));
			}
		},
		Conn::Failed(e) => {
			body.push(text("tt-step-text", format!("That didn't work: {e}")));
			body.push(pill("Try again", Msg::ConnectSpotify));
		},
		Conn::NeedsLogin => body.push(pill("Connect Spotify", Msg::ConnectSpotify)),
		_ => {},
	}
	steps = steps.child(step(icons, 3, "Connect your library", status(account_done, app_done), body));
	steps.into()
}

fn step(icons: &Icons, n: u32, title: &str, status: Step, body: Vec<Node<Msg>>) -> Node<Msg> {
	let class = match status {
		Step::Done => "tt-step tt-done",
		Step::Current => "tt-step",
		Step::Todo => "tt-step tt-todo",
	};
	let marker: Node<Msg> = if status == Step::Done {
		div("tt-step-n").child(icon(icons, Glyph::Check, 16.0, Tap::NONE)).into()
	} else {
		text("tt-step-n", n.to_string())
	};
	let mut content = div("tt-step-body").key("body").child(text("tt-step-title", title));
	if status != Step::Done {
		content = content.children(body);
	}
	div(class).key(format!("step{n}")).child(marker).child(content).into()
}

fn link(label: &str, url: &str) -> Node<Msg> {
	ui::text(vec![ui::span(label).style("link").href(url)]).into()
}

fn pill(label: &str, msg: Msg) -> Node<Msg> {
	html::div().child(html::el("button").class("tt-pill").text(label).on_click(msg)).into()
}

fn field(state: &State, which: Field, value: &str, placeholder: &str, save: Msg) -> Node<Msg> {
	let focused = state.focus == Some(which);
	let input = ui::input()
		.key(which.id())
		.text(value.to_owned())
		.placeholder(if focused { String::new() } else { format!("Paste your {placeholder}") })
		.on_focus(Msg::Focus(Some(which)))
		.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
	div("tt-field").key("field").child(input).child(html::el("button").class("tt-pill").text("Save").on_click(save)).into()
}

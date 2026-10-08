//! Folder changes have no Web API equivalent and never fall back without a Session.

use super::{Request, Response, Session};
use crate::spotify;

pub(super) fn request(s: &mut Session, request: Request, deliver: &dyn Fn(Response)) -> spotify::Result<Option<Response>> {
	let player = s.player.as_mut().ok_or_else(|| spotify::Error::Parse("Start the player before editing playlist folders.".into()))?;
	let response = match request {
		Request::CreateFolder => {
			let name = "New Folder".to_owned();
			let id = player.create_folder(&name)?;
			Response::FolderCreated { id, name }
		},
		Request::RenameFolder(id, name) => {
			player.rename_folder(&id, &name)?;
			Response::Done(format!("Renamed folder to {name}"))
		},
		Request::DeleteFolder(id) => {
			player.delete_folder(&id)?;
			Response::Done("Deleted folder; its contents moved up one level".into())
		},
		Request::MoveToFolder { uri, folder } => {
			player.move_to_folder(&uri, folder.as_deref())?;
			Response::Done("Moved in Your Library".into())
		},
		_ => return Ok(None),
	};
	s.playlists = s.load_playlists()?;
	deliver(Response::Playlists(s.playlists.clone()));
	publish(s, deliver)?;
	Ok(Some(response))
}

pub(super) fn publish(s: &mut Session, deliver: &dyn Fn(Response)) -> spotify::Result<()> {
	if let Some(player) = &mut s.player {
		deliver(Response::Rootlist { account: s.me.clone(), entries: player.rootlist()? });
	}
	Ok(())
}

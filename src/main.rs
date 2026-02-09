use clap::Parser;
use crossterm::{
	event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
	terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::OwoColorize;
use ratatui::{
	Terminal,
	backend::CrosstermBackend,
	layout::{Alignment, Constraint, Direction, Layout},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Clear, Gauge, Padding, Paragraph, Wrap},
};
use snapcast_control::{
	ConnectionStatus, SnapcastConnection, State as SnapcastState, StateGroup as SnapcastGroup,
	client::Client as SnapcastClient, client::ClientVolume,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::time::SystemTime;
use supports_unicode::Stream;
use tabular::{Row, Table};
use tokio::time::{Duration, Sleep};
use tracing;
use tracing_subscriber::EnvFilter;

const EXPECTED_RESPONSE_TIME: Duration = Duration::from_millis(200);
const SUSPICIOUS_QUIET_TIME: Duration = Duration::from_mins(5);
const SUSPEND_MONITOR_TIME: Duration = Duration::from_secs(1);
const SUSPEND_THRESHOLD_TIME: Duration = Duration::from_secs(10);

fn get_binds_table() -> Table {
	struct Bind {
		keys: String,
		description: String,
	}
	let ellipsis = if supports_unicode::on(Stream::Stdout) { "…" } else { "..." };
	let binds = vec![
		Bind {
			keys: format!("{}/{}", "↑".bold(), "↓".bold()),
			description: "navigate up and down (with shift to jump to groups)".to_string(),
		},
		Bind {
			keys: format!("{}/{}", "←".bold(), "→".bold()),
			description: "adjust volume (with shift for larger increments)".to_string(),
		},
		Bind {
			keys: format!("{}/{}/{}/{}", "h".bold(), "j".bold(), "k".bold(), "l".bold()),
			description: format!(
				"same as {}/{}/{}/{}",
				"←".bold(),
				"↓".bold(),
				"↑".bold(),
				"→".bold()
			),
		},
		Bind {
			keys: format!(
				"{}/{}/{}/{}/{}",
				"1".bold(),
				"2".bold(),
				ellipsis,
				"9".bold(),
				"0".bold()
			),
			description: format!("snap volume to 10%, 20%, {}, 90%, 100%", ellipsis),
		},
		Bind { keys: "m".bold().to_string(), description: "toggle mute".to_string() },
		Bind {
			keys: format!("{}/{}/{}", "q".bold(), "Esc".bold(), "^C".bold()),
			description: "quit".to_string(),
		},
	];
	let mut table = Table::new("{:<}  {:<}");
	for entry in binds.iter() {
		table.add_row(Row::new().with_ansi_cell(&entry.keys).with_ansi_cell(&entry.description));
	}
	return table;
}

#[derive(Parser)]
#[command(
	name = "snapmixer",
	version,
	about,
	author,
	after_help = std::iter::once("Keys:".bold().underline().to_string())
		.chain(get_binds_table().to_string().lines().map(|l| format!("  {}", l)))
		.join("\n"),
)]
struct Args {
	#[arg(
		short,
		long,
		value_name = "HOST[:PORT]",
		default_value = "localhost:1705",
		help = "Snapcast server"
	)]
	server: String,
}

struct AppState {
	focus: Option<String>,
	fractional_volumes: HashMap<String, f64>, // client_id -> fractional volume
	error_messages: Vec<String>,
	connected: bool,
	reconnect_attempts: u32,
	connection_stale: bool,
}

impl AppState {
	fn new() -> Self {
		Self {
			focus: None,
			fractional_volumes: HashMap::new(),
			error_messages: Vec::new(),
			connected: false,
			reconnect_attempts: 0,
			connection_stale: false,
		}
	}

	fn update_fractional_volumes(&mut self, snapcast_state: &SnapcastState) {
		for entry in snapcast_state.clients.iter() {
			let client_id = entry.key();
			let current_volume = entry.value().config.volume.percent;
			let fractional =
				self.fractional_volumes.get(client_id.as_str()).copied().unwrap_or(-1.0);
			if current_volume != fractional.round() as usize {
				self.fractional_volumes.insert(client_id.clone(), current_volume as f64);
			}
		}
	}
}

fn get_all_focusable_ids(snapcast_state: &SnapcastState) -> Vec<String> {
	let mut ids = Vec::new();
	for group in sort_groups(snapcast_state).iter() {
		ids.push(group.id.clone());
		for client in sort_clients(group, snapcast_state) {
			ids.push(client.id.clone());
		}
	}
	return ids;
}

fn move_focus(
	delta: i16,
	app_state: &AppState,
	snapcast_state: &SnapcastState,
) -> Option<AppState> {
	let focusable_ids = get_all_focusable_ids(&snapcast_state);

	let fallback = {
		let current_index = if delta > 0 { -1 } else { focusable_ids.len() as i16 };
		let target_index = (current_index + delta).clamp(0, focusable_ids.len() as i16 - 1);
		Some(focusable_ids[target_index as usize].clone())
	};

	let new_focus = match &app_state.focus {
		None => fallback,
		Some(current_focus) => match focusable_ids.iter().position(|s| s == current_focus) {
			None => fallback,
			Some(current_index) => {
				let target_index = current_index as i16 + delta;
				if target_index < 0 {
					if current_index > 0 {
						focusable_ids.first().cloned()
					} else {
						return None;
					}
				} else if target_index >= focusable_ids.len() as i16 {
					if current_index < focusable_ids.len() {
						focusable_ids.last().cloned()
					} else {
						return None;
					}
				} else {
					Some(focusable_ids[target_index as usize].clone())
				}
			}
		},
	};

	if new_focus != app_state.focus {
		return Some(AppState {
			focus: new_focus,
			fractional_volumes: app_state.fractional_volumes.clone(),
			error_messages: app_state.error_messages.clone(),
			connected: app_state.connected,
			reconnect_attempts: app_state.reconnect_attempts,
			connection_stale: app_state.connection_stale,
		});
	}
	return None;
}

fn get_group_id_of_client(client_id: String, snapcast_state: &SnapcastState) -> Option<String> {
	return sort_groups(snapcast_state).iter().find_map(|group| {
		if group.clients.contains(&client_id) {
			return Some(group.id.clone());
		}
		return None;
	});
}

fn move_focus_group(
	delta: i16,
	app_state: &AppState,
	snapcast_state: &SnapcastState,
) -> Option<AppState> {
	let focusable_ids: Vec<String> =
		sort_groups(snapcast_state).iter().map(|group| group.id.clone()).collect();

	let fallback = {
		let current_index = if delta > 0 { -1 } else { focusable_ids.len() as i16 };
		let target_index = (current_index + delta).clamp(0, focusable_ids.len() as i16 - 1);
		Some(focusable_ids[target_index as usize].clone())
	};

	let new_focus = match &app_state.focus {
		None => fallback,
		Some(current_focus) => match focusable_ids.iter().position(|s| s == current_focus) {
			None => match get_group_id_of_client(current_focus.to_string(), &snapcast_state) {
				None => fallback,
				Some(current_group_id) => {
					match focusable_ids.iter().position(|t| t == &current_group_id) {
						None => fallback,
						Some(parent_group_index) => {
							let target_index = (parent_group_index as i16
								+ delta + (if delta > 0 { 0 } else { 1 }))
							.clamp(0, focusable_ids.len() as i16);
							Some(focusable_ids[target_index as usize].clone())
						}
					}
				}
			},
			Some(current_index) => {
				let target_index = current_index as i16 + delta;
				if target_index < 0 {
					if current_index > 0 {
						focusable_ids.first().cloned()
					} else {
						return None;
					}
				} else if target_index >= focusable_ids.len() as i16 {
					if current_index < focusable_ids.len() {
						focusable_ids.last().cloned()
					} else {
						return None;
					}
				} else {
					Some(focusable_ids[target_index as usize].clone())
				}
			}
		},
	};

	if new_focus != app_state.focus {
		return Some(AppState {
			focus: new_focus,
			fractional_volumes: app_state.fractional_volumes.clone(),
			error_messages: app_state.error_messages.clone(),
			connected: app_state.connected,
			reconnect_attempts: app_state.reconnect_attempts,
			connection_stale: app_state.connection_stale,
		});
	}
	return None;
}

async fn set_volume(
	volume: f64,
	app_state: &mut AppState,
	snapcast_state: &SnapcastState,
	snapcast_client: &mut SnapcastConnection,
) -> bool {
	let target_volume = volume.clamp(0.0, 100.0);
	let id = match app_state.focus.as_ref() {
		Some(id) => id,
		None => return false,
	};

	if let Some(group) = snapcast_state.groups.get(id) {
		let group_clients: Vec<SnapcastClient> = snapcast_state
			.clients
			.iter()
			.filter(|entry| group.clients.contains(entry.key()))
			.map(|entry| entry.value().clone())
			.collect();

		if group_clients.len() == 0 {
			return false;
		}

		// Find loudest client
		let loudest_fractional = group_clients
			.iter()
			.map(|client| {
				app_state
					.fractional_volumes
					.get(&client.id)
					.copied()
					.unwrap_or(client.config.volume.percent as f64)
			})
			.max_by(|a, b| a.partial_cmp(b).unwrap())
			.unwrap_or(0.0);

		if loudest_fractional == 0.0 {
			// Avoid division by zero
			for client in group_clients.iter() {
				app_state.fractional_volumes.insert(client.id.clone(), target_volume);
				let _ = snapcast_client
					.client_set_volume(
						client.id.to_string(),
						ClientVolume {
							percent: target_volume.round() as usize,
							..client.config.volume
						},
					)
					.await;
			}
		} else {
			// Scale proportionally using fractional volumes
			let factor = target_volume / loudest_fractional;
			for client in group_clients.iter() {
				let current_fractional = app_state
					.fractional_volumes
					.get(&client.id)
					.copied()
					.unwrap_or(client.config.volume.percent as f64);
				let new_fractional = (current_fractional * factor).clamp(0.0, 100.0);
				app_state.fractional_volumes.insert(client.id.clone(), new_fractional);

				let _ = snapcast_client
					.client_set_volume(
						client.id.to_string(),
						ClientVolume {
							percent: new_fractional.round() as usize,
							..client.config.volume
						},
					)
					.await;
			}
		}
		return true;
	} else if let Some(client) = snapcast_state.clients.get(id) {
		app_state.fractional_volumes.insert(client.id.clone(), target_volume);
		let _ = snapcast_client
			.client_set_volume(
				client.id.to_string(),
				ClientVolume { percent: target_volume.round() as usize, ..client.config.volume },
			)
			.await;
		return true;
	}

	return false;
}

async fn set_volume_delta(
	delta: f64,
	app_state: &mut AppState,
	snapcast_state: &SnapcastState,
	snapcast_client: &mut SnapcastConnection,
) -> bool {
	let id = match app_state.focus.as_ref() {
		Some(id) => id,
		None => return false,
	};

	let current_volume = if let Some(group) = snapcast_state.groups.get(id) {
		// Find loudest client in group using fractional volumes
		snapcast_state
			.clients
			.iter()
			.filter(|entry| group.clients.contains(entry.key()))
			.map(|entry| {
				app_state
					.fractional_volumes
					.get(entry.key())
					.copied()
					.unwrap_or(entry.value().config.volume.percent as f64)
			})
			.max_by(|a, b| a.partial_cmp(b).unwrap())
	} else {
		// Single client
		snapcast_state.clients.get(id).map(|entry| {
			app_state
				.fractional_volumes
				.get(entry.key())
				.copied()
				.unwrap_or(entry.value().config.volume.percent as f64)
		})
	};

	if let Some(current) = current_volume {
		let target_volume = (current + delta).clamp(0.0, 100.0);
		return set_volume(target_volume, app_state, snapcast_state, snapcast_client).await;
	}

	return false;
}

fn parse_server(s: &str) -> Result<(String, u16), String> {
	match s.rsplit_once(":") {
		Some((host, port)) if !port.is_empty() => {
			let port = port.parse::<u16>().map_err(|_| format!("Invalid port number {}", port))?;
			Ok((host.to_string(), port))
		}
		_ => Ok((s.to_string(), 1705)),
	}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	// Set up tracing
	tracing_subscriber::fmt()
		.with_env_filter(
			EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off")),
		)
		.with_writer(std::io::stderr)
		.init();

	let args = Args::parse();
	let (host, port) = parse_server(&args.server)?;
	let addr_str = format!("{}:{}", host, port);

	tracing::debug!("Looking up {}", addr_str);
	let socket_addr = tokio::net::lookup_host(&addr_str)
		.await
		.map_err(|e| format!("DNS lookup failed: {}", e))?
		.next()
		.ok_or_else(|| format!("DNS lookup returned no addresses for {}", &addr_str))?;

	let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();

	tracing::debug!("Connecting to Snapcast server");
	let mut snapcast_client = SnapcastConnection::builder()
		.on_status_change({
			let tx = status_tx.clone();
			move |status| {
				let _ = tx.send(status);
			}
		})
		.connect(socket_addr)
		.await
		.map_err(|e| format!("Couldn't connect to Snapcast server: {}", e))?;

	// Set up terminal
	tracing::debug!("Setting up terminal");
	let mut stdout = std::io::stdout();
	crossterm::execute!(stdout, EnterAlternateScreen)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;
	enable_raw_mode()?;

	let mut input = EventStream::new();

	let snapcast_state = snapcast_client.state.clone();
	let mut app_state = AppState::new();

	// Set up timers for connection and suspension monitoring
	let mut no_receive_timeout: Option<Pin<Box<Sleep>>> =
		Some(Box::pin(tokio::time::sleep(SUSPICIOUS_QUIET_TIME)));
	let mut no_response_timeout: Option<Pin<Box<Sleep>>> = None;
	let mut last_wall_time = SystemTime::now();
	let mut suspend_monitor_interval = tokio::time::interval(SUSPEND_MONITOR_TIME);

	loop {
		let mut needs_redraw = false;
		let mut sent = false;
		let mut received = false;

		tokio::select! {
			_ = suspend_monitor_interval.tick() => {
				let wall_time = SystemTime::now();
				if let Ok(delta) = wall_time.duration_since(last_wall_time) {
					if delta >= SUSPEND_THRESHOLD_TIME {
						tracing::debug!("Possible system suspend/resume detected: expected ~1 sec to have passed; in fact {:?} secs have passed", delta);
						let _ = snapcast_client.server_get_status().await;
						sent = true;
					}
				}
				last_wall_time = wall_time;
			}

			_ = async {
				if let Some(timer) = &mut no_receive_timeout {
					timer.as_mut().await;
				}
			}, if no_receive_timeout.is_some() && app_state.connected && !app_state.connection_stale => {
				tracing::debug!("No messages received for a while; requesting status");
				no_receive_timeout = None;
				let _ = snapcast_client.server_get_status().await;
				sent = true;
			}

			_ = async {
				if let Some(timer) = &mut no_response_timeout {
					timer.as_mut().await;
				}
			}, if no_response_timeout.is_some() && app_state.connected && !app_state.connection_stale => {
				tracing::debug!("No response; marking connection stale");
				app_state.connection_stale = true;
				needs_redraw = true;
			}

			Some(status) = status_rx.recv() => {
				tracing::debug!("Connection status changed to {:?}", status);
				match status {
					ConnectionStatus::Connected => {
						app_state.connected = true;
						app_state.reconnect_attempts = 0;
						let _ = snapcast_client.server_get_status().await;
						needs_redraw = true;
					}
					ConnectionStatus::Disconnected => {
						app_state.connected = false;
						app_state.reconnect_attempts = 1;
						needs_redraw = true;
					}
					ConnectionStatus::ReconnectFailed => {
						app_state.reconnect_attempts += 1;
						needs_redraw = true;
					}
				}
			}

			Some(messages) = snapcast_client.recv() => {
				tracing::debug!("Received {} messages from Snapcast server", messages.len());
				received = true;
				if app_state.connection_stale {
					app_state.connection_stale = false;
					needs_redraw = true;
				}
				for message in messages {
					match message {
						Ok(_) => {
							app_state.update_fractional_volumes(&snapcast_state);
							needs_redraw = true;
						}
						Err(err) => {
							app_state.error_messages.push(format!("{}", err));
							needs_redraw = true;
						}
					}
				}
			},

			maybe_event = input.next() => {
				tracing::trace!("Received keyboard event");
				if let Some(Ok(event)) = maybe_event {
					match event {
						Event::Key(key) => match handle_key(key, &app_state) {
							Action::Exit => break,
							Action::Dismiss => {
								if app_state.error_messages.is_empty() {
									// No errors to dismiss; dismiss the whole app
									break;
								} else {
									app_state.error_messages.clear();
									needs_redraw = true;
								}
							}
							Action::Prev => {
								if let Some(new_state) = move_focus(-1, &app_state, &snapcast_state) {
									app_state = new_state;
									needs_redraw = true;
								}
							}
							Action::Next => {
								if let Some(new_state) = move_focus(1, &app_state, &snapcast_state) {
									app_state = new_state;
									needs_redraw = true;
								}
							},
							Action::PrevGroup => {
								if let Some(new_state) = move_focus_group(-1, &app_state, &snapcast_state) {
									app_state = new_state;
									needs_redraw = true;
								}
							},
							Action::NextGroup => {
								if let Some(new_state) = move_focus_group(1, &app_state, &snapcast_state) {
									app_state = new_state;
									needs_redraw = true;
								}
							},
							Action::ReduceVolume => {
								sent = set_volume_delta(-1.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::ReduceVolumeMore => {
								sent = set_volume_delta(-5.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::RaiseVolume => {
								sent = set_volume_delta(1.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::RaiseVolumeMore => {
								sent = set_volume_delta(5.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo10 => {
								sent = set_volume(10.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo20 => {
								sent = set_volume(20.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo30 => {
								sent = set_volume(30.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo40 => {
								sent = set_volume(40.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo50 => {
								sent = set_volume(50.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo60 => {
								sent = set_volume(60.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo70 => {
								sent = set_volume(70.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo80 => {
								sent = set_volume(80.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo90 => {
								sent = set_volume(90.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::SetVolumeTo100 => {
								sent = set_volume(100.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
							},
							Action::ToggleMute => {
								if let Some(id) = app_state.focus.as_ref() {
									if let Some(group) = snapcast_state.groups.get(id) {
										let _ = snapcast_client.group_set_mute(group.id.to_string(), !group.muted).await;
										sent = true;
									} else if let Some(client) = snapcast_state.clients.get(id) {
										let _ = snapcast_client.client_set_volume(client.id.to_string(), ClientVolume {
											muted: !client.config.volume.muted,
											..client.config.volume
										}).await;
										sent = true;
									}
								}
							},
							Action::None => {},
						}
						Event::Resize(_, _) => needs_redraw = true,
						_ => {}
					}
				}
			}
		}

		if received {
			tracing::trace!("Resetting received timer, cancelling response timer");
			no_receive_timeout = Some(Box::pin(tokio::time::sleep(SUSPICIOUS_QUIET_TIME)));
			no_response_timeout = None;
		};

		if sent {
			tracing::trace!("Resetting response timer");
			no_response_timeout = Some(Box::pin(tokio::time::sleep(EXPECTED_RESPONSE_TIME)));
		};

		if needs_redraw {
			draw_ui(&mut terminal, &app_state, &snapcast_state);
		}
	}

	// Clean up
	disable_raw_mode()?;
	crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
	terminal.show_cursor()?;
	Ok(())
}

enum Action {
	Dismiss,
	Exit,
	Prev,
	PrevGroup,
	Next,
	NextGroup,
	ReduceVolume,
	ReduceVolumeMore,
	RaiseVolume,
	RaiseVolumeMore,
	SetVolumeTo10,
	SetVolumeTo20,
	SetVolumeTo30,
	SetVolumeTo40,
	SetVolumeTo50,
	SetVolumeTo60,
	SetVolumeTo70,
	SetVolumeTo80,
	SetVolumeTo90,
	SetVolumeTo100,
	ToggleMute,
	None,
}

fn handle_key(key: KeyEvent, app_state: &AppState) -> Action {
	if key.kind != KeyEventKind::Press {
		return Action::None;
	}

	if !app_state.connected || app_state.connection_stale {
		match key.code {
			KeyCode::Char('q') => Action::Exit,
			KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Exit,
			_ => Action::None,
		}
	} else if !app_state.error_messages.is_empty() {
		match key.code {
			KeyCode::Esc => Action::Dismiss,
			_ => Action::None,
		}
	} else {
		match key.code {
			// Dismiss
			KeyCode::Esc => Action::Dismiss,

			// Exit
			KeyCode::Char('q') => Action::Exit,
			KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Exit,

			// Move between groups
			KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => Action::PrevGroup,
			KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => Action::NextGroup,
			KeyCode::Char('K') => Action::PrevGroup,
			KeyCode::Char('J') => Action::NextGroup,

			// Move to neighbouring rows
			KeyCode::Up => Action::Prev,
			KeyCode::Down => Action::Next,
			KeyCode::Char('k') => Action::Prev,
			KeyCode::Char('j') => Action::Next,

			// Volume down
			KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
				Action::ReduceVolumeMore
			}
			KeyCode::Char('H') => Action::ReduceVolumeMore,
			KeyCode::Left => Action::ReduceVolume,
			KeyCode::Char('h') => Action::ReduceVolume,

			// Volume up
			KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
				Action::RaiseVolumeMore
			}
			KeyCode::Char('L') => Action::RaiseVolumeMore,
			KeyCode::Right => Action::RaiseVolume,
			KeyCode::Char('l') => Action::RaiseVolume,

			// Snap volume
			KeyCode::Char('1') => Action::SetVolumeTo10,
			KeyCode::Char('2') => Action::SetVolumeTo20,
			KeyCode::Char('3') => Action::SetVolumeTo30,
			KeyCode::Char('4') => Action::SetVolumeTo40,
			KeyCode::Char('5') => Action::SetVolumeTo50,
			KeyCode::Char('6') => Action::SetVolumeTo60,
			KeyCode::Char('7') => Action::SetVolumeTo70,
			KeyCode::Char('8') => Action::SetVolumeTo80,
			KeyCode::Char('9') => Action::SetVolumeTo90,
			KeyCode::Char('0') => Action::SetVolumeTo100,

			// Mute
			KeyCode::Char('m') => Action::ToggleMute,

			_ => Action::None,
		}
	}
}

fn get_group_name(group: &SnapcastGroup) -> String {
	if group.name.is_empty() {
		return format!("Group with ID {}", group.id);
	}
	return group.name.clone();
}

fn get_client_name(client: &SnapcastClient) -> String {
	if client.config.name.is_empty() {
		if client.host.name.is_empty() {
			return format!("Client with ID {}", client.id);
		}
		return format!("Client on host {}", client.host.name);
	}
	return client.config.name.clone();
}

fn get_longest_client_name_length(snapcast_state: &SnapcastState) -> usize {
	snapcast_state.clients.iter().map(|c| get_client_name(&c).len()).max().unwrap_or(0)
}

fn get_volume_symbol(muted: bool) -> Span<'static> {
	let symbol = {
		if supports_unicode::on(Stream::Stdout) {
			if muted { "🔇" } else { "🔊" }
		} else {
			if muted { "M" } else { " " }
		}
	};
	return Span::styled(
		symbol,
		Style::default().fg(if muted { Color::Red } else { Color::Green }),
	);
}

fn sort_groups(snapcast_state: &SnapcastState) -> Vec<SnapcastGroup> {
	let mut groups: Vec<_> = snapcast_state.groups.iter().map(|g| g.clone()).collect();
	groups.sort_by(|a, b| {
		let name_a = get_group_name(a);
		let name_b = get_group_name(b);
		name_a.cmp(&name_b)
	});
	return groups;
}

fn sort_clients(group: &SnapcastGroup, snapcast_state: &SnapcastState) -> Vec<SnapcastClient> {
	let mut clients: Vec<_> = group
		.clients
		.iter()
		.filter_map(|id| snapcast_state.clients.get(id).map(|c| c.clone()))
		.collect();
	clients.sort_by(|a, b| {
		let name_a = get_client_name(a);
		let name_b = get_client_name(b);
		name_a.cmp(&name_b)
	});
	return clients;
}

fn render_modal(
	frame: &mut ratatui::Frame,
	title: &str,
	message: &str,
	border_color: Color,
	subtitle: Option<&str>,
) {
	let area = frame.area().centered(Constraint::Percentage(80), Constraint::Percentage(50));
	frame.render_widget(Clear, area);

	let mut block = Block::bordered()
		.border_style(Style::default().fg(border_color))
		.border_type(ratatui::widgets::BorderType::Rounded)
		.padding(Padding::new(1, 1, 0, 0))
		.title(Span::styled(
			format!(" {} ", title),
			Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD),
		));

	if let Some(subtitle) = subtitle {
		block = block.title(Line::from(format!(" {} ", subtitle)).right_aligned());
	}

	frame.render_widget(&block, area);
	let inner = block.inner(area);
	let paragraph = Paragraph::new(message).wrap(Wrap { trim: false });
	frame.render_widget(paragraph, inner);
}

fn draw_ui(
	terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
	app_state: &AppState,
	snapcast_state: &SnapcastState,
) {
	terminal
		.draw(|frame| {
			let groups = sort_groups(snapcast_state);

			// Set up main layout and reserve space for each group
			let groups_layout = Layout::default()
				.direction(Direction::Vertical)
				.constraints(groups.iter().map(|group| {
					let len = group.clients.len() as u16;
					Constraint::Length(len + 2) // +2 for top/bottom borders
				}))
				.split(frame.area());

			let longest_client_name_length = get_longest_client_name_length(&snapcast_state);

			// Render each group
			for (index, group) in groups.iter().enumerate() {
				// Put together full title
				let title_style = if app_state.focus.as_deref() == Some(&group.id) {
					Style::default()
				} else {
					Style::default().fg(Color::Reset)
				};
				let block_title = Line::from(vec![
					get_volume_symbol(group.muted),
					Span::raw(" "),
					Span::styled(get_group_name(group), title_style.add_modifier(Modifier::BOLD)),
					Span::raw(" "),
				]);

				// Group block
				let block = Block::bordered()
					.border_style(Style::default().fg(
						if app_state.focus.as_deref() == Some(&group.id) {
							Color::Yellow
						} else {
							Color::Indexed(236)
						},
					))
					.border_type(ratatui::widgets::BorderType::Rounded)
					.padding(Padding::new(1, 1, 0, 0))
					.title(block_title);
				frame.render_widget(&block, groups_layout[index]);

				// Sort clients by name
				let clients = sort_clients(group, snapcast_state);

				// Render each client
				let block_inner = block.inner(groups_layout[index]);
				let client_constraints = vec![Constraint::Length(1); clients.len()];
				let client_rows = Layout::vertical(client_constraints).split(block_inner);
				for (index, client) in clients.iter().enumerate() {
					let client_row = client_rows[index];

					// Styled name
					let client_name = get_client_name(&client);
					let name_span = if app_state.focus.as_deref() == Some(&client.id) {
						Span::styled(
							client_name,
							Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
						)
					} else {
						Span::raw(client_name)
					};

					// Volume gauge
					let gauge = Gauge::default()
						.ratio(client.config.volume.percent as f64 / 100.0)
						.gauge_style(Style::default().fg(
							if app_state.focus.as_deref() == Some(&client.id) {
								Color::Yellow
							} else if group.muted || client.config.volume.muted {
								Color::Indexed(238)
							} else {
								Color::Blue
							},
						));

					// Lay out the parts
					let parts = Layout::horizontal([
						Constraint::Length(longest_client_name_length as u16), // name
						Constraint::Length(1),                                 // gap
						Constraint::Length(2),                                 // mute
						Constraint::Length(1),                                 // gap
						Constraint::Min(10),                                   // gauge
					])
					.split(client_row);
					frame.render_widget(
						Paragraph::new(Line::from(vec![name_span]).alignment(Alignment::Right)),
						parts[0],
					);
					frame.render_widget(
						Paragraph::new(Line::from(vec![get_volume_symbol(
							client.config.volume.muted,
						)])),
						parts[2],
					);
					frame.render_widget(gauge, parts[4]);
				}
			}

			if !app_state.error_messages.is_empty() {
				render_modal(
					frame,
					"Error",
					&app_state.error_messages.join("\n"),
					Color::Red,
					Some("esc to dismiss"),
				);
			} else if !app_state.connected {
				render_modal(
					frame,
					"Connection status",
					&format!(
						"Disconnected. Attempting to reconnect...\nReconnection attempt: {}",
						app_state.reconnect_attempts
					),
					Color::Yellow,
					None,
				);
			} else if app_state.connection_stale {
				render_modal(
					frame,
					"Connection status",
					"Connection appears to be stale. Awaiting response...",
					Color::Yellow,
					None,
				);
			}
		})
		.unwrap();
}

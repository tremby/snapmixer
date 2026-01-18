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
	SnapcastConnection, State as SnapcastState, client::Client as SnapcastClient,
	client::ClientVolume,
};
use std::collections::HashMap;
use supports_unicode::Stream;
use tabular::{Row, Table};
use tokio;

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
				"h".bold(),
				"j".bold(),
				"k".bold(),
				"l".bold()
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
	disable_help_flag = true,
	after_help = std::iter::once("Keys:".bold().underline().to_string())
		.chain(get_binds_table().to_string().lines().map(|l| format!("  {}", l)))
		.join("\n"),
)]
struct Args {
	#[arg(short, long, default_value = "localhost", help = "Snapcast server hostname or IP")]
	host: String,

	#[arg(short, long, default_value_t = 1705, help = "Snapcast server port")]
	port: u16,

	#[arg(long, action = clap::ArgAction::Help, help = "Print help")]
	help: Option<bool>,
}

struct AppState {
	focus: Option<String>,
	fractional_volumes: HashMap<String, f64>, // client_id -> fractional volume
	error_messages: Vec<String>,
}

impl AppState {
	fn new() -> Self {
		Self { focus: None, fractional_volumes: HashMap::new(), error_messages: Vec::new() }
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
	for entry in snapcast_state.groups.iter() {
		ids.push(entry.key().clone());
		for client_id in &entry.value().clients {
			ids.push(client_id.clone());
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
		});
	}
	return None;
}

fn get_group_id_of_client(client_id: String, snapcast_state: &SnapcastState) -> Option<String> {
	return snapcast_state.groups.iter().find_map(|entry| {
		let (group_id, group) = entry.pair();
		if group.clients.contains(&client_id) {
			return Some(group_id.clone());
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
		snapcast_state.groups.iter().map(|entry| entry.key().clone()).collect();

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
		});
	}
	return None;
}

async fn set_volume(
	volume: f64,
	app_state: &mut AppState,
	snapcast_state: &SnapcastState,
	snapcast_client: &mut SnapcastConnection,
) {
	let target_volume = volume.clamp(0.0, 100.0);
	let id = match app_state.focus.as_ref() {
		Some(id) => id,
		None => return,
	};
	if let Some(group) = snapcast_state.groups.get(id) {
		let group_clients: Vec<SnapcastClient> = snapcast_state
			.clients
			.iter()
			.filter(|entry| group.clients.contains(entry.key()))
			.map(|entry| entry.value().clone())
			.collect();

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
	} else if let Some(client) = snapcast_state.clients.get(id) {
		app_state.fractional_volumes.insert(client.id.clone(), target_volume);
		let _ = snapcast_client
			.client_set_volume(
				client.id.to_string(),
				ClientVolume { percent: target_volume.round() as usize, ..client.config.volume },
			)
			.await;
	}
}

async fn set_volume_delta(
	delta: f64,
	app_state: &mut AppState,
	snapcast_state: &SnapcastState,
	snapcast_client: &mut SnapcastConnection,
) {
	let id = match app_state.focus.as_ref() {
		Some(id) => id,
		None => return,
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
		set_volume(target_volume, app_state, snapcast_state, snapcast_client).await;
	}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args = Args::parse();

	let addr = format!("{}:{}", args.host, args.port);
	let socket_addr = tokio::net::lookup_host(&addr)
		.await
		.map_err(|e| format!("DNS lookup failed: {}", e))?
		.next()
		.ok_or("Couldn't connect to Snapcast server")?;

	// SnapcastConnection::open doesn't return a Result which means its panic on error is hard to
	// deal with. So instead of that, test the connection first.
	tokio::net::TcpStream::connect(socket_addr)
		.await
		.map_err(|e| format!("Couldn't connect to {}:{}: {}", args.host, args.port, e))?;

	let mut snapcast_client = SnapcastConnection::open(socket_addr).await;

	snapcast_client.server_get_status().await.expect("Could not send request");

	// Set up terminal
	let mut stdout = std::io::stdout();
	crossterm::execute!(stdout, EnterAlternateScreen)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;
	enable_raw_mode()?;

	let mut input = EventStream::new();

	let snapcast_state = snapcast_client.state.clone();
	let mut app_state = AppState::new();

	loop {
		let mut needs_redraw = false;

		tokio::select! {
				Some(message) = snapcast_client.recv() => {
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
				},

				maybe_event = input.next() => {
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
														let _ = set_volume_delta(-1.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::ReduceVolumeMore => {
														let _ = set_volume_delta(-5.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::RaiseVolume => {
														let _ = set_volume_delta(1.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::RaiseVolumeMore => {
														let _ = set_volume_delta(5.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo10 => {
														let _ = set_volume(10.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo20 => {
														let _ = set_volume(20.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo30 => {
														let _ = set_volume(30.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo40 => {
														let _ = set_volume(40.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo50 => {
														let _ = set_volume(50.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo60 => {
														let _ = set_volume(60.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo70 => {
														let _ = set_volume(70.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo80 => {
														let _ = set_volume(80.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo90 => {
														let _ = set_volume(90.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::SetVolumeTo100 => {
														let _ = set_volume(100.0, &mut app_state, &snapcast_state, &mut snapcast_client).await;
												},
												Action::ToggleMute => {
														if let Some(id) = app_state.focus.as_ref() {
																if let Some(group) = snapcast_state.groups.get(id) {
																		let _ = snapcast_client.group_set_mute(group.id.to_string(), !group.muted).await;
																} else if let Some(client) = snapcast_state.clients.get(id) {
																		let _ = snapcast_client.client_set_volume(client.id.to_string(), ClientVolume {
																				muted: !client.config.volume.muted,
																				..client.config.volume
																		}).await;
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

	if !app_state.error_messages.is_empty() {
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

fn get_client_name(client: &SnapcastClient) -> String {
	if client.config.name.is_empty() {
		if !client.host.name.is_empty() {
			return format!("Client on host {}", client.host.name);
		}
		return format!("Client with ID {}", client.id);
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

fn draw_ui(
	terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
	app_state: &AppState,
	snapcast_state: &SnapcastState,
) {
	terminal
		.draw(|frame| {
			// Set up main layout and reserve space for each group
			let groups_layout = Layout::default()
				.direction(Direction::Vertical)
				.constraints(snapcast_state.groups.iter().map(|group| {
					let len = group.value().clients.len() as u16;
					Constraint::Length(len + 2) // +2 for top/bottom borders
				}))
				.split(frame.area());

			let longest_client_name_length = get_longest_client_name_length(&snapcast_state);

			// Render each group
			for (index, entry) in snapcast_state.groups.iter().enumerate() {
				let (group_id, group) = (entry.key(), entry.value());

				// Decide on group display name
				let group_name = if group.name.is_empty() {
					format!("Group with ID {}", group_id)
				} else {
					group.name.clone()
				};

				// Put together full title
				let title_style = if app_state.focus.as_deref() == Some(&group.id) {
					Style::default()
				} else {
					Style::default().fg(Color::Reset)
				};
				let block_title = Line::from(vec![
					get_volume_symbol(group.muted),
					Span::raw(" "),
					Span::styled(group_name, title_style.add_modifier(Modifier::BOLD)),
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

				// Render each client
				let block_inner = block.inner(groups_layout[index]);
				let client_constraints = vec![Constraint::Length(1); group.clients.len()];
				let client_rows = Layout::vertical(client_constraints).split(block_inner);
				for (index, client_id) in group.clients.iter().enumerate() {
					let client_row = client_rows[index];
					let client = match snapcast_state.clients.get(client_id) {
						Some(c) => c,
						None => continue,
					};

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
				let error_area =
					frame.area().centered(Constraint::Percentage(80), Constraint::Percentage(50));
				frame.render_widget(Clear, error_area);
				let error_block = Block::bordered()
					.border_style(Style::default().fg(Color::Red))
					.border_type(ratatui::widgets::BorderType::Rounded)
					.padding(Padding::new(1, 1, 0, 0))
					.title(Span::styled(
						" Error ",
						Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD),
					))
					.title(Line::from(" esc to dismiss ").right_aligned());
				frame.render_widget(&error_block, error_area);
				let error_block_inner = error_block.inner(error_area);
				let paragraph =
					Paragraph::new(app_state.error_messages.join("\n")).wrap(Wrap { trim: false });
				frame.render_widget(paragraph, error_block_inner);
			}
		})
		.unwrap();
}

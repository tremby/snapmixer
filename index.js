const blessed = require('neo-blessed');
const {Console} = require('console');
const yargs = require('yargs/yargs');
const { hideBin } = require('yargs/helpers');

const SnapcastClient = require('./snapcast-client');

const logger = new Console(process.stderr, process.stderr);

async function main(argv) {
	const options = yargs(hideBin(process.argv))
		.usage('Control snapcast volumes.')
		.option('h', {
			alias: 'host',
			default: 'localhost',
			requiresArg: true,
		})
		.option('p', {
			alias: 'port',
			default: 1705,
			number: true,
			requiresArg: true,
		})
		.parse();

	const client = new SnapcastClient({
		host: options.host,
		port: options.port,
	});
	client.on('message', (message) => {
		// TODO: handle notifications in a more sophisticated way
		logger.log("got a message:", message);
		updateDisplay();
	});
	await client.connect();

	const screen = blessed.screen({
		smartCSR: true,
		dockBorders: true,
	});
	screen.title = "Snapmixer";

	const form = blessed.form({
		position: {
			left: 0,
			top: 0,
			width: '100%',
			height: '100%',
		},
		scrollable: true,
		scrollbar: {
			style: {
				bg: '#999',
			},
			track: {
				bg: '#333',
			},
		},
		keys: true,
		vi: true,
		mouse: true,
	});
	screen.append(form);

	const groupBoxes = {};

	async function updateDisplay() {
		const response = await client.getStatus();
		const groups = response.server.groups;
		let formY = 0;
		for (const group of groups) {
			let groupY = 0;
			let groupSpec = groupBoxes[group.id];
			if (!groupSpec) {
				groupSpec = groupBoxes[group.id] = {
					box: blessed.box({
						position: {
							left: 0,
							top: formY,
							width: '100%-1',
							// height: set later
						},
						border: 'line',
						style: {
							border: {
								fg: '#333',
							},
							label: {
								fg: '#666',
								bold: true,
								position: {
									left: 16,
								},
								left: 16,
								rleft: 16,
							},
						},
						tags: true,
						// label: set later
					}),
					clients: {},
				};
				form.append(groupSpec.box);
			}

			if (group.muted || group.name.length) {
				groupSpec.box.setLabel(` ${group.name}${group.muted ? `${group.name.length ? ' ' : ''}{red-fg}(muted){/}` : ''} `);
			} else {
				groupSpec.box.removeLabel();
			}

			for (const client of group.clients) {
				let clientSpec = groupSpec.clients[client.id];
				if (!clientSpec) {
					clientSpec = groupSpec.clients[client.id] = {
						label: blessed.text({
							position: {
								left: 0,
								top: groupY,
								width: 16,
								height: 1,
							},
							style: {
								// fg: set later
							},
							// content: set later
						}),
						muteStatus: blessed.text({
							position: {
								left: 17,
								top: groupY,
								width: 1,
								height: 1,
							},
							style: {
								fg: 'red',
								bold: true,
							},
							// content: set later
						}),
						bar: blessed.progressbar({
							pch: '\u2591',
							style: {
								bg: '#333',
								bar: {
									bg: '#666',
									fg: '#ccc',
								},
								focus: {
									bg: 'blue',
									bar: {
										bg: 'lightblue',
										fg: 'white',
									},
								},
							},
							position: {
								width: `100%-${19 + 2 + 1}`,
								height: 1,
								top: groupY,
								left: 19,
							},
							// filled: set later
							input: true,
						}),
					};
					groupSpec.box.append(clientSpec.label);
					groupSpec.box.append(clientSpec.muteStatus);
					groupSpec.box.append(clientSpec.bar);

					// Store the client and group IDs on the progress bar
					// for easy access
					clientSpec.bar.clientId = client.id;
					clientSpec.bar.groupId = group.id;
				}
				clientSpec.label.setContent(client.config.name.length ? client.config.name : client.host.name);
				clientSpec.label.style.fg = client.config.name.length ? 'white' : '#999';
				clientSpec.muteStatus.setContent(client.config.volume.muted ? "M" : "");
				clientSpec.bar.setProgress(client.config.volume.percent);
				groupY += 2;
			}

			groupSpec.box.position.height = groupY + 1; // Group's bottom border
			formY += groupSpec.box.position.height;
		}

		// TODO: handle clients and groups getting removed

		screen.render();
	}

	const helpMessage = blessed.message({
		hidden: true,
		position: {
			width: Math.min(screen.width, 80),
			height: Math.min(screen.height, 26),
			left: 'center',
			top: 'center',
		},
		border: 'line',
		label: " Help ",
		scrollable: true,
		style: {
			border: {
				fg: '#333',
			},
			label: {
				fg: '#666',
			},
		},
		scrollbar: {
			style: {
				bg: '#999',
			},
			track: {
				bg: '#333',
			},
		},
		keys: true,
		vi: true,
		mouse: true,
	});
	screen.append(helpMessage);
	helpMessage.append(blessed.table({
		transparent: true,
		tags: true,
		data: [
			["{bold}?{/bold}, {bold}F1{/bold}", "Toggle this help box"],
			["{bold}down{/bold}, {bold}up{/bold}", "Select mixer, scroll help"],
			["{bold}j{/bold}, {bold}k{/bold}", "Select mixer, scroll help"],
			["{bold}tab{/bold}, {bold}shift-tab{/bold}", "Select mixer"],
			["{bold}left{/bold}, {bold}right{/bold}", "Adjust volume"],
			["{bold}h{/bold}, {bold}l{/bold}", "Adjust volume"],
			["{bold}shift-left{/bold}, {bold}shift-right{/bold}", "Adjust volume in large increments"],
			["{bold}H{/bold}, {bold}L{/bold}", "Adjust volume in large increments"],
			["{bold}1{/bold}, {bold}2{/bold}, {bold}3{/bold}, ..., {bold}0{/bold}", "Set volume to 10%, 20%, 30%, ..., 100%"],
			["{bold}m{/bold}", "Toggle client mute"],
			["{bold}g{/bold}", "Toggle group mute"],
			["{bold}esc{/bold}, {bold}q{/bold}, {bold}control-c{/bold}", "Quit"],
		],
		position: {
			width: '100%-3' /* left and right border, plus scrollbar */,
		},
	}));

	// Quit
	screen.key(['escape', 'q', 'C-c'], (ch, key) => {
		client.close();
		process.exit(0);
	});

	// Help
	screen.key(['?', 'f1'], (ch, key) => {
		helpMessage.toggle();
		if (!helpMessage.hidden) {
			helpMessage.focus();
			helpMessage.resetScroll();
		}
		screen.render();
	});

	// Adjust volume in small increments
	screen.key(['right', 'l'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.adjustVolume(widget.clientId, 1);
		updateDisplay();
	});
	screen.key(['left', 'h'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.adjustVolume(widget.clientId, -1);
		updateDisplay();
	});

	// Adjust volume in large increments
	screen.key(['S-right', 'S-l'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.adjustVolume(widget.clientId, 3);
		updateDisplay();
	});
	screen.key(['S-left', 'S-h'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.adjustVolume(widget.clientId, -3);
		updateDisplay();
	});

	// Snap volume to 10%, 20%, 30%, ..., 100%
	for (let i = 0; i < 10; i++) {
		screen.key([i.toString()], async (ch, key) => {
			if (!helpMessage.hidden) {
				return;
			}
			const widget = screen.focused;
			if (widget.type !== 'progress-bar') {
				return;
			}
			await client.setVolume(widget.clientId, (i === 0 ? 10 : i) * 10);
			updateDisplay();
		});
	}

	// Toggle client mute
	screen.key(['m'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.toggleClientMute(widget.clientId);
		updateDisplay();
	});

	// Toggle group mute
	screen.key(['g'], async (ch, key) => {
		if (!helpMessage.hidden) {
			return;
		}
		const widget = screen.focused;
		if (widget.type !== 'progress-bar') {
			return;
		}
		await client.toggleGroupMute(widget.groupId);
		updateDisplay();
	});

	updateDisplay();
}

main(process.argv);

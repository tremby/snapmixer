const events = require('events');
const jsonMultiParse = require('json-multi-parse');
const net = require('net');
const uuidv4 = require('uuid/v4');

class JsonRpcClient extends events.EventEmitter {
	constructor(options) {
		super();
		this.options = options;
		this.connected = false;
		this.connectionOk = false;
		this.buffer = '';
		this.promiseResolvers = {};
	}

	async connect() {
		return new Promise((resolve, reject) => {
			this.client = net.createConnection(this.options, () => {
				this.connected = true;
				this.connectionOk = true;
				this.client.on('error', (error) => {
					this.connectionOk = false;
					throw new Error(error);
				});
				this.client.on('close', () => {
					this.connected = false;
					this.connectionOk = false;
				});
				resolve();
			});

			this.client.once('error', (error) => {
				this.connectionOk = false;
				if (!this.connected) {
					reject(error);
				} else {
					throw error;
				}
			});

			this.client.setEncoding('utf8');

			this.client.on('data', (data) => {
				// Append to existing buffer
				this.buffer += data;

				// Parse objects out
				const objects = jsonMultiParse(this.buffer, {
					partial: true,
				});

				// Keep remainder for next time
				this.buffer = objects.remainder;

				// Deal with messages
				for (const message of objects) {
					if (message.id && this.promiseResolvers[message.id]) {
						// This is a response we expected;
						// resolve the corresponding promise
						this.promiseResolvers[message.id](message);
						delete this.promiseResolvers[message.id];
					} else {
						// This is an unexpected message; emit an event
						this.emit('message', message);
					}
				}
			});
		});
	}

	async close() {
		return new Promise((resolve, reject) => {
			this.client.end(() => {
				this.client.destroy();
				resolve();
			});
		});
	}

	async send(method, params, notification = false) {
		return new Promise((resolve, reject) => {
			if (!this.connectionOk) {
				reject("Connection not OK");
				return;
			}

			const message = {
				jsonrpc: '2.0',
				method: method,
			};

			if (params) {
				message.params = params;
			}

			if (!notification) {
				message.id = uuidv4();
				this.promiseResolvers[message.id] = resolve;
			}

			this.client.write(JSON.stringify(message) + "\r\n");

			if (notification) {
				resolve();
			}
		});
	}
}

module.exports = JsonRpcClient;

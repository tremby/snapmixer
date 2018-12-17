const JsonRpcClient = require('./json-rpc-client');

class SnapcastClient extends JsonRpcClient {
	async getStatus() {
		const response = await this.send('Server.GetStatus');
		return response.result;
	}

	async setClientMute(clientId, muted) {
		const response = await this.send('Client.SetVolume', {
			id: clientId,
			volume: {
				muted: muted,
			},
		});
		return response.result;
	}

	async getClientStatus(clientId) {
		const response = await this.send('Client.GetStatus', {
			id: clientId,
		});
		return response.result.client;
	}

	async getClientMute(clientId) {
		return (await this.getClientStatus(clientId)).config.volume.muted;
	}

	async toggleClientMute(clientId) {
		return this.setClientMute(clientId, !await this.getClientMute(clientId));
	}

	async getVolume(clientId) {
		return (await this.getClientStatus(clientId)).config.volume.percent;
	}

	async setVolume(clientId, volume) {
		const response = await this.send('Client.SetVolume', {
			id: clientId,
			volume: {
				percent: Math.min(100, Math.max(0, volume)),
			},
		});
		return response.result;
	}

	async adjustVolume(clientId, delta) {
		return this.setVolume(clientId, (await this.getVolume(clientId)) + delta);
	}

	async getGroupStatus(groupId) {
		const response = await this.send('Group.GetStatus', {
			id: groupId,
		});
		return response.result.group;
	}

	async getGroupMute(groupId) {
		return (await this.getGroupStatus(groupId)).muted;
	}

	async setGroupMute(groupId, muted) {
		const response = await this.send('Group.SetMute', {
			id: groupId,
			mute: muted,
		});
		return response.result;
	}

	async toggleGroupMute(groupId) {
		return this.setGroupMute(groupId, !await this.getGroupMute(groupId));
	}
}

module.exports = SnapcastClient;

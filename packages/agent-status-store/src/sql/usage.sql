CREATE TABLE IF NOT EXISTS usage_observations (
	agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
	observation_id TEXT NOT NULL,
	subagent_id TEXT NOT NULL,
	kind TEXT NOT NULL CHECK(kind IN ('skill','command','mcp')),
	name TEXT NOT NULL,
	server TEXT NOT NULL,
	operation TEXT NOT NULL CHECK(operation IN ('invoke','load','request')),
	observed_at INTEGER NOT NULL,
	source TEXT NOT NULL,
	event_type TEXT NOT NULL,
	tool_call_id TEXT NOT NULL,
	PRIMARY KEY(agent_id,observation_id)
);
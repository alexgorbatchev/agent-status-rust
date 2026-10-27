WITH ranked AS (
		SELECT *, ROW_NUMBER() OVER w AS rank,
		COUNT(*) OVER p AS count, MIN(observed_at) OVER p AS first_used_at, MAX(observed_at) OVER p AS last_used_at
		FROM usage_observations WHERE agent_id=?
		WINDOW p AS (PARTITION BY kind,server,name,operation),
		w AS (p ORDER BY observed_at DESC,observation_id DESC)
	) SELECT kind,name,server,operation,count,first_used_at,last_used_at,
		source,event_type,observed_at,subagent_id,tool_call_id FROM ranked WHERE rank=1
		ORDER BY kind,server,name,operation
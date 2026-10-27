	SELECT
			COALESCE(SUM(CASE WHEN r.id IS NOT NULL AND r.ended_at IS NULL THEN 1 ELSE 0 END), 0) AS active_count,
			COALESCE(SUM(CASE 
				WHEN r.id IS NOT NULL AND r.ended_at IS NULL AND e.last_heartbeat_at > 0 AND (? - e.last_heartbeat_at) <= 30000 AND (
					a.activity IN ('WAITING_QUESTION', 'ERROR') OR
					(a.activity = 'BUSY' AND a.active_tool IS NOT NULL AND a.active_tool != '' AND a.active_tool_started_at > 0 AND (? - a.active_tool_started_at) >= 180000) OR
					(a.activity = 'BUSY' AND a.updated_at > 0 AND (? - a.updated_at) >= 120000) OR
					(a.activity = 'RATE_LIMITED' AND a.updated_at > 0 AND (? - a.updated_at) >= 120000 AND (
						(a.rate_limit_reset_at IS NOT NULL AND a.rate_limit_reset_at <= ?) OR
						(a.rate_limit_reset_at IS NULL AND (a.retry_after_seconds IS NULL OR (? - a.updated_at) >= (a.retry_after_seconds * 1000)))
					))
				) THEN 1 ELSE 0 END), 0) AS attention_count,
			SUM(a.cost_usd) AS total_spend
		FROM agents a LEFT JOIN runs r ON a.current_run_id=r.id
		LEFT JOIN environments e ON a.environment_id = e.id
	
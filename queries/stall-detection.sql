-- Injection stall detection: windows where no new pods became Running.
-- Parameters: {experiment_id}
WITH windowed AS (
  SELECT
    tumbling_window(timestamp_ms, 5000) as window_ms,
    MAX(CAST(running AS INT)) as max_running
  FROM events
  WHERE event_type = 'POLL_TICK'
    AND experiment_id = '{experiment_id}'
  GROUP BY tumbling_window(timestamp_ms, 5000)
)
SELECT
  window_ms,
  max_running,
  max_running - LAG(max_running) OVER (ORDER BY window_ms) as delta_running
FROM windowed
ORDER BY window_ms

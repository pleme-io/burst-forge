-- Prediction variance analysis: correlate prediction error with GW memory pressure
-- Usage: Replace {experiment_id} with your experiment ID
SELECT
    e.scenario,
    e.predicted_min_secs,
    CAST(e.elapsed_ms AS DOUBLE) / 1000.0 as actual_secs,
    e.prediction_error_pct,
    e.prediction_verdict,
    m.metric_value as gw_memory_bytes,
    CASE
        WHEN m.metric_value > 500000000 THEN 'MEMORY_PRESSURE'
        WHEN e.prediction_error_pct > 50 THEN 'FORMULA_MISS'
        WHEN e.prediction_error_pct > 20 THEN 'CONTENTION'
        ELSE 'NOMINAL'
    END as variance_reason
FROM events e
LEFT JOIN events m
    ON asof_nearest(e.timestamp_ms, m.timestamp_ms, 30000) IS NOT NULL
    AND m.signal_type = 'metric'
    AND m.metric_name = 'go_memstats_alloc_bytes'
WHERE e.experiment_id = '{experiment_id}'
    AND e.event_type = 'BURST_COMPLETE'
    AND e.predicted_gw_replicas IS NOT NULL
ORDER BY e.prediction_error_pct DESC

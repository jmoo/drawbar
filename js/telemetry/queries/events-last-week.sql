-- Rows of each kind in the last seven days, through the Analytics Engine SQL API.
-- Weight by _sample_interval: Analytics Engine may sample at high volume.
SELECT index1 AS event, SUM(_sample_interval) AS rows
FROM drawbar_events
WHERE timestamp > NOW() - INTERVAL '7' DAY
GROUP BY event
ORDER BY rows DESC;

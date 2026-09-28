-- Visits and first-today visitors per day. For `visit`, blob2 is `navigation` and blob3
-- `first_day` (schema.json's field order, then its `edge` fields).
SELECT toStartOfInterval(timestamp, INTERVAL '1' DAY) AS day,
       SUM(IF(blob2 = 'navigate', _sample_interval, 0)) AS visits,
       SUM(IF(blob3 = '1', _sample_interval, 0)) AS visitors
FROM drawbar_events
WHERE index1 = 'visit'
GROUP BY day
ORDER BY day;

-- Reports, newest first. Run in the D1 console for drawbar-reports.
SELECT id, datetime(created, 'unixepoch') AS sent, kind, version, model, firmware,
       browser, os, contact, text
FROM reports
ORDER BY created DESC
LIMIT 50;

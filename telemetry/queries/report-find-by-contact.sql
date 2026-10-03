-- What is held for one contact address, for an access or erasure request.
SELECT id, datetime(created, 'unixepoch') AS sent, kind, text
FROM reports
WHERE contact = 'someone@example.com';

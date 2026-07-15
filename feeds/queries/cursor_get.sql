-- Read a feed's saved resume position, or nothing if it has never run.
SELECT cursor
FROM feed_cursors
WHERE feed = $1;

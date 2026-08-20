CREATE TRIGGER governor_events_single_terminal
BEFORE INSERT ON governor_events
WHEN NEW.event_type IN ('attempt_completed', 'attempt_failed')
 AND EXISTS (
    SELECT 1
    FROM governor_events
    WHERE attempt_id = NEW.attempt_id
      AND event_type IN ('attempt_completed', 'attempt_failed')
 )
BEGIN
    SELECT RAISE(ABORT, 'arbiter: duplicate terminal event');
END;

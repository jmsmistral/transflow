-- Operational pause intervals fence delayed event/clock observations after resume.
CREATE TABLE schedule_pause_intervals (
    schedule_id TEXT NOT NULL REFERENCES schedules(id),
    trigger_epoch TEXT NOT NULL,
    started_at_us INTEGER NOT NULL CHECK(started_at_us>=0),
    event_after INTEGER NOT NULL CHECK(event_after>=0),
    ended_at_us INTEGER CHECK(ended_at_us>=started_at_us),
    event_through INTEGER CHECK(event_through>=event_after),
    CHECK((ended_at_us IS NULL)=(event_through IS NULL)),
    PRIMARY KEY(schedule_id,trigger_epoch,event_after,started_at_us)
) STRICT;
CREATE UNIQUE INDEX schedule_open_pause ON schedule_pause_intervals(schedule_id,trigger_epoch) WHERE ended_at_us IS NULL;

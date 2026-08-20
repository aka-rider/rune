//! Multiprocess scenarios split into append/race and crash/gc/divergence groups.

#[path = "scenarios_append_race.rs"]
mod append_race;

#[path = "scenarios_crash_gc_divergence.rs"]
mod crash_gc_divergence;

.pragma library

// One-frame window previews use short screencasts. Qualify each stream separately
// so a sequence of thumbnails cannot keep the recording indicator visible.
function update(casts, previous, now) {
    const entries = {};
    const visibleCasts = [];
    let nextDelay = 0;

    for (const cast of casts) {
        const key = String(cast.stream_id);
        const entry = previous[key] || { since: now, visible: false };
        const visible = entry.visible || (cast.is_active && now - entry.since >= 1000);
        if (!visible && !cast.is_active)
            continue;

        entries[key] = { since: entry.since, visible: visible };
        if (visible) {
            visibleCasts.push(cast);
            continue;
        }

        const remaining = Math.max(1, 1000 - (now - entry.since));
        nextDelay = nextDelay > 0 ? Math.min(nextDelay, remaining) : remaining;
    }

    return { entries: entries, casts: visibleCasts, nextDelay: nextDelay };
}

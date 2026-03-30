#!/bin/bash
# Monitor Reddit posts for new comments. Run via cron or loop.
# Usage: ./monitor-reddit.sh
# State file tracks last seen comments to avoid duplicates.

STATE_FILE="${HOME}/.cache/atomic-rollback-reddit-state"
NOTIFY_CMD="notify-send"  # KDE Plasma desktop notification

mkdir -p "$(dirname "$STATE_FILE")"
touch "$STATE_FILE"

fetch_comments() {
    local url="$1"
    local label="$2"
    curl -sL -H "User-Agent: atomic-rollback-monitor/0.1" "${url}.json?limit=25" 2>/dev/null | \
    python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
    # Post listing format: data[1] has comments
    if isinstance(data, list) and len(data) > 1:
        comments = data[1]['data']['children']
    else:
        comments = data['data']['children']
    for c in comments:
        if c['kind'] != 't1': continue
        d = c['data']
        cid = d['id']
        author = d.get('author', '[deleted]')
        body = d.get('body', '')[:200]
        print(f'{cid}\t{author}\t{body}')
except Exception as e:
    print(f'ERROR\t\t{e}', file=sys.stderr)
" 2>/dev/null
}

check_post() {
    local url="$1"
    local label="$2"

    fetch_comments "$url" "$label" | while IFS=$'\t' read -r cid author body; do
        if [ -z "$cid" ] || grep -qF "$cid" "$STATE_FILE" 2>/dev/null; then
            continue
        fi
        echo "$cid" >> "$STATE_FILE"
        echo "[$label] $author: $body"
        $NOTIFY_CMD "atomic-rollback: $label" "$author: ${body:0:100}" 2>/dev/null
    done
}

# Add your post URLs here after posting
# r/fedora post
check_post "https://www.reddit.com/r/Fedora/comments/1s7bcft/i_got_tired_of_fedora_having_no_rollback_so_i" "r/fedora"
check_post "https://www.reddit.com/r/rust/comments/1s7bcv0/i_got_tired_of_fedora_having_no_rollback_so_i" "r/rust"

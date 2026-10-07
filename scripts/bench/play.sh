#!/bin/bash
# Explicitly invoking this script without --list transfers the active Spotify player.
set -euo pipefail
exec python3 - "$@" <<'PY'
import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

parser = argparse.ArgumentParser(description='Control a Spotify Connect device through the Web API using SPOTIFY_ACCESS_TOKEN.')
parser.add_argument('--device-name', default='terntable')
parser.add_argument('--list', action='store_true', help='List devices; do not transfer or start playback.')
parser.add_argument('--pause', action='store_true', help='Pause playback on the device.')
parser.add_argument('--resume', action='store_true', help='Resume playback on the device.')
parser.add_argument('track', nargs='?', help='spotify:track:ID, open.spotify.com/track/ID, or 22-character track ID')
args = parser.parse_args()
if args.list and args.track:
    parser.error('--list does not take a track')
if not (args.list or args.pause or args.resume) and not args.track:
    parser.error('provide a track, or use --list/--pause/--resume')
track_id = None
if args.track:
    match = re.fullmatch(r'(?:spotify:track:|https://open\.spotify\.com/track/)?([A-Za-z0-9]{22})(?:\?[^\s]*)?', args.track)
    if not match:
        parser.error('expected a Spotify track URI, URL, or 22-character ID')
    track_id = match.group(1)

try:
    access_token = os.environ.get('SPOTIFY_ACCESS_TOKEN', '').strip()
    if not access_token:
        raise RuntimeError('Set SPOTIFY_ACCESS_TOKEN to a current Web API token with user-read-playback-state and user-modify-playback-state scopes.')

    def request(method, url, body=None):
        headers = {'Authorization': 'Bearer ' + access_token}
        if body is not None:
            body = json.dumps(body).encode()
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(url, data=body, headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=30) as response:
            raw = response.read()
            try:
                return json.loads(raw) if raw else None
            except ValueError:
                return None

    base = 'https://api.spotify.com/v1/me/player'
    devices = request('GET', base + '/devices')['devices']
    if args.list:
        for device in devices:
            print(json.dumps({key: device.get(key) for key in ('id', 'name', 'type', 'is_active', 'is_restricted', 'volume_percent')}))
        sys.exit(0)
    matches = [device for device in devices if device.get('name') == args.device_name]
    if len(matches) != 1:
        names = ', '.join(str(device.get('name')) for device in devices) or '(none)'
        raise RuntimeError(f'Expected exactly one {args.device_name!r} device, found {len(matches)}. Available: {names}. Run only one librespot build at a time.')
    device = matches[0]
    if not device.get('id') or device.get('is_restricted'):
        raise RuntimeError('Selected device has no ID or is restricted from Web API playback')
    if args.pause or args.resume:
        request('PUT', base + ('/pause?' if args.pause else '/play?') + urllib.parse.urlencode({'device_id': device['id']}))
        print('paused' if args.pause else 'resumed')
        sys.exit(0)
    print('Transferring playback to ' + args.device_name, file=sys.stderr)
    request('PUT', base, {'device_ids': [device['id']], 'play': True})
    request('PUT', base + '/play?' + urllib.parse.urlencode({'device_id': device['id']}), {'uris': ['spotify:track:' + track_id]})
    print('Started spotify:track:' + track_id + ' on ' + args.device_name)
except urllib.error.HTTPError as error:
    # Never print request headers or response bodies (which may contain secrets).
    print(f'Spotify HTTP {error.code} from {error.url.split("?")[0]}. Check token expiry/scopes, Premium, and device availability.', file=sys.stderr)
    sys.exit(1)
except (OSError, ValueError, KeyError, RuntimeError, urllib.error.URLError) as error:
    print(f'play.sh: {error}', file=sys.stderr)
    sys.exit(1)
PY

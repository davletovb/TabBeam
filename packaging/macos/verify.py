"""Verify an installed package, including a real Native Messaging round trip."""
import json
import pathlib
import struct
import subprocess
import sys

extension_id = sys.argv[1]
host = pathlib.Path('/Library/Application Support/Pervue/pervue-host')
manifest = pathlib.Path('/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json')
data = json.loads(manifest.read_text())
assert data['name'] == 'com.pervue.host'
assert data['path'] == str(host)
assert data['allowed_origins'] == [f'chrome-extension://{extension_id}/']
assert host.is_file()

request = json.dumps(dict(version=1, type='request', request_id='package_status',
                          method='provider.status', payload={'provider_id': 'codex'})).encode()
with subprocess.Popen([str(host), f'chrome-extension://{extension_id}/'],
                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
    output, stderr = proc.communicate(struct.pack('<I', len(request)) + request, timeout=20)
    assert proc.returncode == 0, stderr.decode(errors='replace')
events = []
while output:
    assert len(output) >= 4
    length = struct.unpack('<I', output[:4])[0]
    events.append(json.loads(output[4:4 + length]))
    output = output[4 + length:]
assert events[0]['event'] == 'host.ready'
assert 1 in events[0]['payload']['protocol_versions']
assert any(e['event'] == 'provider.status' and e['request_id'] == 'package_status' for e in events)
assert events[-1]['event'] == 'response.completed'
print('Installed package registration, host protocol and status round trip verified')

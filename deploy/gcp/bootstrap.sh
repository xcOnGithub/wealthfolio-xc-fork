#!/bin/bash
# Run only after inspecting the existing VM, seeding its original key, and staging
# the nonsecret metadata with manage.py install. Does not format or replace disks.
set -Eeuo pipefail
umask 077
stage=$(mktemp -d /run/wealthfolio-deploy.XXXXXX)
trap 'rm -rf "$stage"' EXIT

command -v docker >/dev/null
docker compose version >/dev/null
command -v python3 >/dev/null
command -v flock >/dev/null
# Serialize manual bootstrap with image releases before either can stop the app.
exec 9>/run/lock/wealthfolio-release.lock
flock -x 9

# The VM's existing Docker installation is reused; no daemon/package upgrades.
python3 - "$stage" <<'PY'
import base64, importlib.util, json, os, pathlib, shutil, socket, subprocess, sys, urllib.request
stage = pathlib.Path(sys.argv[1])
metadata = 'http://metadata.google.internal/computeMetadata/v1/'
def read(path):
    request = urllib.request.Request(metadata + path, headers={'Metadata-Flavor': 'Google'})
    with urllib.request.urlopen(request, timeout=20) as response: return json.load(response)
config = read('instance/attributes/wealthfolio-config')
files = read('instance/attributes/wealthfolio-files')
required = {'compose.yaml', 'Caddyfile', 'fetch-secrets.py', 'release.py', 'block-metadata.sh', 'wealthfolio.service', 'bootstrap.sh'}
if set(files) != required: raise SystemExit('Unexpected deployment bundle')
for name, value in files.items(): (stage/name).write_bytes(base64.b64decode(value, validate=True))
spec = importlib.util.spec_from_file_location('credentials', stage/'fetch-secrets.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
project_request = urllib.request.Request(metadata+'project/project-id', headers={'Metadata-Flavor':'Google'})
with urllib.request.urlopen(project_request, timeout=20) as response: project = response.read().decode()
module.validate_config(config, project)
data = pathlib.Path(config['data_dir'])
if not data.is_dir() or not any(data.iterdir()):
    raise SystemExit('Existing installation data is missing; refusing cutover')
backup_root=pathlib.Path('/var/backups/wealthfolio')
if data.resolve()==backup_root or data.resolve() in backup_root.parents:
    raise SystemExit('Backup directory must be outside the application data')
required_space=sum(p.stat().st_size for p in data.rglob('*') if p.is_file() and not p.is_symlink())
if shutil.disk_usage('/var/backups').free < required_space + 64*1024*1024:
    raise SystemExit('Insufficient disk space for the pre-cutover data backup')
container = config['previous_container']
record = json.loads(subprocess.check_output(['docker','inspect',container]))[0]
running = subprocess.check_output(['docker','ps','-q'],text=True).split()
records=json.loads(subprocess.check_output(['docker','inspect',*running])) if running else []
for other in records:
    if other['Id']==record['Id']: continue
    image=other['Config']['Image']
    if not image.startswith(('caddy:','caddy@','docker.io/library/caddy:','docker.io/library/caddy@')):
        raise SystemExit('Other workloads use this VM; review metadata isolation before installing the dedicated-VM service')
unit_active=subprocess.run(['systemctl','is-active','--quiet','wealthfolio.service']).returncode==0
directory=''
if unit_active:
    directory=subprocess.check_output(['systemctl','show','wealthfolio.service','--property=WorkingDirectory','--value'],text=True).strip()
    compose_directory=(record['Config'].get('Labels') or {}).get('com.docker.compose.project.working_dir')
    if not directory or directory not in {compose_directory,'/opt/wealthfolio'}:
        raise SystemExit('Cannot establish ownership of the existing wealthfolio.service')
if config['manage_proxy']:
    owned=[]
    if unit_active and directory=='/opt/wealthfolio':
        for other in records:
            labels=other['Config'].get('Labels') or {}
            if labels.get('com.docker.compose.project')=='wealthfolio' and labels.get('com.docker.compose.service')=='proxy': owned.append(other)
    for port in (80,443):
        owned_port=any(binding.get('HostPort')==str(port) for other in owned for binding in (other['HostConfig'].get('PortBindings') or {}).get(str(port)+'/tcp',[]))
        if owned_port: continue
        with socket.socket() as probe:
            try: probe.bind(('0.0.0.0',port))
            except OSError: raise SystemExit('An unrelated proxy owns HTTP/HTTPS; reuse it instead of replacing it')
mounts = [m for m in record.get('Mounts',[]) if m['Destination']=='/data']
if len(mounts)!=1 or pathlib.Path(mounts[0]['Source']).resolve()!=data.resolve():
    raise SystemExit('Existing container data mount differs from the deployment plan')
if mounts[0]['Type']=='volume':
    if config.get('data_volume')!=mounts[0]['Name']:
        raise SystemExit('Preserve the existing Docker volume name in the deployment plan')
    volume=json.loads(subprocess.check_output(['docker','volume','inspect',mounts[0]['Name']]))[0]
    if volume['Driver']!='local' or volume.get('Options'):
        raise SystemExit('Driver-managed storage needs a separate consistent-backup plan before cutover')
elif mounts[0]['Type']!='bind' or config.get('data_volume') is not None:
    raise SystemExit('Unsupported or mismatched application data mount')
env = dict(v.split('=',1) for v in record['Config'].get('Env',[]) if '=' in v)
if env.get('WF_DB_PATH','/data/wealthfolio.db')!='/data/'+config['db_filename']:
    raise SystemExit('Database path differs from the deployment plan')
old_key = env.get('WF_SECRET_KEY','').strip().encode()
if env.get('WF_SECRET_KEY_FILE'):
    old_key = subprocess.check_output(['docker','exec','--user','0',container,'cat',env['WF_SECRET_KEY_FILE']],stderr=subprocess.DEVNULL).strip()
if not old_key: raise SystemExit('Cannot verify the existing master key')
token = read('instance/service-accounts/default/token')['access_token']
module.load(config, project, token, runtime=stage/'runtime')
import hmac
if not hmac.compare_digest(old_key, (stage/'runtime/master-key').read_bytes().strip()):
    raise SystemExit('Secret Manager master key differs from the running installation')
docker_env = dict(os.environ, DOCKER_CONFIG=str(stage/'runtime/docker'))
subprocess.run(['docker','pull',config['image']],env=docker_env,check=True)
if config['manage_proxy']: subprocess.run(['docker','pull',config['proxy_image']],env=docker_env,check=True)
subprocess.run(['docker','run','--rm','--network','none','--mount','type=bind,src='+str(data)+',dst=/data',
    '--entrypoint','sh',config['image'],'-c','test -r /data && test -w /data && test -x /data'],check=True)
# Keep the complete pre-cutover plan privately for recovery, without key values.
(stage/'config.json').write_text(json.dumps(config))
(stage/'previous-container.json').write_text(json.dumps({'name':container,'restart':record['HostConfig']['RestartPolicy']}))
PY

# Stop only the selected app. The existing reverse proxy is left alone unless
# manage_proxy explicitly selected this package's proxy and its ports are free.
previous=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["previous_container"])' "$stage/config.json")
docker update --restart=no "$previous" >/dev/null
if systemctl is-active --quiet wealthfolio.service; then systemctl stop wealthfolio.service; fi
if docker inspect "$previous" >/dev/null 2>&1; then docker stop "$previous" >/dev/null; fi

python3 - "$stage" <<'PY'
import datetime, importlib.util, json, pathlib, shutil, sys, tarfile
stage = pathlib.Path(sys.argv[1]); config=json.loads((stage/'config.json').read_text())
data=pathlib.Path(config['data_dir']); backup=pathlib.Path('/var/backups/wealthfolio')
if data==backup or data in backup.parents: raise SystemExit('Backup directory must be outside the application data')
backup.mkdir(mode=0o700, parents=True, exist_ok=True); backup.chmod(0o700)
stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
destination=backup/(stamp+'.tar.gz')
with tarfile.open(destination,'x:gz') as archive: archive.add(data,arcname='data',recursive=True)
shutil.copyfile(stage/'previous-container.json',backup/(stamp+'-container.json'))
if pathlib.Path('/etc/systemd/system/wealthfolio.service').exists():
    shutil.copyfile('/etc/systemd/system/wealthfolio.service',backup/(stamp+'-service'))
if pathlib.Path('/etc/wealthfolio/config.json').exists():
    shutil.copyfile('/etc/wealthfolio/config.json',backup/(stamp+'-config.json'))
install=pathlib.Path('/opt/wealthfolio'); install.mkdir(mode=0o755,parents=True,exist_ok=True)
for name in ['compose.yaml','Caddyfile','fetch-secrets.py','release.py','block-metadata.sh','bootstrap.sh']:
    shutil.copyfile(stage/name,install/name)
    (install/name).chmod(0o750 if name.endswith(('.sh','.py')) else 0o644)
service=(stage/'wealthfolio.service').read_text().replace('@WF_APP_DATA_DIR@',str(data))
pathlib.Path('/etc/systemd/system/wealthfolio.service').write_text(service)
spec=importlib.util.spec_from_file_location('credentials',stage/'fetch-secrets.py')
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
pathlib.Path('/etc/wealthfolio').mkdir(mode=0o700,parents=True,exist_ok=True)
module.atomic_write(pathlib.Path('/etc/wealthfolio/config.json'),json.dumps(config).encode())
for name in ['data','config']:
    pathlib.Path('/srv/wealthfolio/caddy',name).mkdir(mode=0o700,parents=True,exist_ok=True)
print('Pre-cutover data backup:',destination)
PY
systemctl daemon-reload
systemctl enable wealthfolio.service
systemctl start wealthfolio.service
for attempt in $(seq 1 90); do
  state=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' wealthfolio-wealthfolio-1 2>/dev/null || true)
  if [ "$state" = healthy ]; then
    flock -u 9
    if /usr/bin/python3 /opt/wealthfolio/release.py --initialize; then
      echo 'WEALTHFOLIO_BOOTSTRAP_READY: application and HTTPS/auth checks passed'
      exit 0
    fi
    break
  fi
  if systemctl is-failed --quiet wealthfolio.service; then break; fi
  sleep 2
done
systemctl stop wealthfolio.service
echo 'New application failed application or HTTPS/auth verification. It is stopped; the original data backup is preserved for recovery.' >&2
exit 1

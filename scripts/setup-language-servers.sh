#!/bin/sh
# Explicit, opt-in installer. Never called by Pokarak startup. No global JAVA_HOME.
set -eu
exec python3 - "$@" <<'PY'
import argparse, concurrent.futures, hashlib, json, os, pathlib, platform, re
import shutil, subprocess, tempfile, urllib.parse, urllib.request, xml.etree.ElementTree as ET

parser = argparse.ArgumentParser(description='Install isolated Pokarak language tools from official release metadata')
parser.add_argument('--tools-dir', type=pathlib.Path, default=pathlib.Path.home()/'.local/share/pokarak/tools')
parser.add_argument('--update', action='store_true', help='Resolve current official releases instead of reusing installed metadata')
args = parser.parse_args()
if platform.system() != 'Darwin' or platform.machine() not in ('arm64', 'aarch64'):
    raise SystemExit('This setup currently supports Apple Silicon macOS only.')
root = args.tools_dir.expanduser().resolve()
root.mkdir(parents=True, exist_ok=True)


def fetch(url):
    request = urllib.request.Request(url, headers={'User-Agent': 'pokarak-language-tools/1', 'Accept': 'application/json'})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def metadata(url):
    return json.loads(fetch(url))


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix+'.tmp')
    temporary.write_text(json.dumps(value, indent=2)+'\n')
    temporary.replace(path)


def checksum(url, length=64):
    match = re.search(r'\b[0-9a-fA-F]{'+str(length)+r'}\b', fetch(url).decode())
    if not match:
        raise RuntimeError('Missing checksum at '+url)
    return match[0].lower()


def file_digest(path, algorithm='sha256'):
    digest = hashlib.new(algorithm)
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024*1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def archive(name, resolve):
    record = root/(name+'.json')
    if record.exists() and not args.update:
        info = json.loads(record.read_text())
        if pathlib.Path(info['directory']).is_dir() and pathlib.Path(info['directory'], '.complete').is_file():
            print('Reusing '+name+' '+info['version'], flush=True)
            return info
    info = resolve()
    algorithm = info.get('checksumAlgorithm','sha256')
    expected = info.get('checksum',info.get('sha256'))
    target = root/name/info['version']
    target.parent.mkdir(exist_ok=True)
    if not (target/'.complete').exists():
        downloads = root/'downloads'
        downloads.mkdir(exist_ok=True)
        filename = pathlib.Path(urllib.parse.urlparse(info['url']).path).name
        saved = downloads/filename
        if not saved.exists() or file_digest(saved,algorithm) != expected:
            print('Downloading '+name+' '+info['version']+' from '+info['url'], flush=True)
            part = saved.with_suffix(saved.suffix+'.part')
            subprocess.run(['/usr/bin/curl', '--fail', '--location', '--retry', '3', '--connect-timeout', '30', '--max-time', '1800', '--silent', '--show-error', '--output', str(part), info['url']], check=True)
            if file_digest(part,algorithm) != expected:
                part.unlink()
                raise RuntimeError('Checksum mismatch for '+name)
            part.replace(saved)
        stage = pathlib.Path(tempfile.mkdtemp(prefix='.extract-', dir=target.parent))
        try:
            subprocess.run(['/usr/bin/tar', '-xf', str(saved), '-C', str(stage)], check=True)
            (stage/'.complete').write_text(expected+'\n')
            if target.exists():
                raise RuntimeError('Incomplete existing installation; inspect/remove '+str(target))
            stage.rename(target)
        finally:
            if stage.exists(): shutil.rmtree(stage)
    info['directory'] = str(target)
    atomic_json(record, info)
    return info


def jdk_release():
    source = 'https://api.github.com/repos/adoptium/temurin25-binaries/releases/latest'
    release = metadata(source)
    asset = next(a for a in release['assets'] if re.fullmatch(r'OpenJDK25U-jdk_aarch64_mac_hotspot_.*\.tar\.gz', a['name']))
    digest = asset.get('digest') or ('sha256:'+checksum(asset['browser_download_url']+'.sha256.txt'))
    return {'version':release['tag_name'].replace('+','_'), 'metadata':source, 'url':asset['browser_download_url'], 'sha256':digest.removeprefix('sha256:')}


def kotlin_release():
    source = 'https://api.github.com/repos/Kotlin/kotlin-lsp/releases/latest'
    release = metadata(source)
    # Official standalone archive URLs now live in the release body, not assets.
    urls = re.findall(r'https://download\.jetbrains\.com/[^\s)<>]+', release['body'])
    url = next(u for u in urls if u.endswith('-aarch64.sit'))
    return {'version':release['name'].removeprefix('v'), 'metadata':source, 'url':url, 'sha256':checksum(url+'.sha256')}


def jdt_release():
    source = 'https://api.github.com/repos/eclipse-jdtls/eclipse.jdt.ls/tags?per_page=20'
    tags = metadata(source)
    version = next(t['name'][1:] for t in tags if re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+',t['name']))
    base = 'https://download.eclipse.org/jdtls/milestones/'+version+'/'
    filename = fetch(base+'latest.txt').decode().strip()
    if not re.fullmatch(r'jdt-language-server-[0-9.]+-[0-9]+\.tar\.gz',filename):
        raise RuntimeError('Unexpected JDT milestone metadata: '+filename)
    url = base+filename
    return {'version':version, 'metadata':base+'latest.txt', 'url':url, 'sha256':checksum(url+'.sha256')}


def node_release():
    source = 'https://nodejs.org/dist/index.json'
    release = next(r for r in metadata(source) if r['version'].startswith('v22.') and r['lts'])
    version = release['version']
    filename = 'node-'+version+'-darwin-arm64.tar.gz'
    base = 'https://nodejs.org/dist/'+version+'/'
    line = next(line for line in fetch(base+'SHASUMS256.txt').decode().splitlines() if line.endswith('  '+filename))
    return {'version':version, 'metadata':source, 'url':base+filename, 'sha256':line.split()[0]}


def maven_release():
    base = 'https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven/'
    source = base+'maven-metadata.xml'
    versions = [v.text for v in ET.fromstring(fetch(source)).findall('./versioning/versions/version') if re.fullmatch(r'3\.[0-9]+\.[0-9]+',v.text)]
    version = max(versions,key=lambda v:tuple(map(int,v.split('.'))))
    url = base+version+'/apache-maven-'+version+'-bin.tar.gz'
    return {'version':version,'metadata':source,'url':url,'checksumAlgorithm':'sha512','checksum':checksum(url+'.sha512',128)}


with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    futures = {name:pool.submit(archive,name,resolve) for name,resolve in [('jdk25',jdk_release),('kotlin-lsp',kotlin_release),('jdtls',jdt_release),('node',node_release),('maven',maven_release)]}
    installed = {name:future.result() for name,future in futures.items()}


def unique(base, pattern):
    hits = list(pathlib.Path(base).glob(pattern))
    if len(hits) != 1:
        raise RuntimeError('Expected one '+pattern+' under '+base+', found '+str(hits))
    return hits[0].resolve()


java = unique(installed['jdk25']['directory'],'*/Contents/Home/bin/java')
node = unique(installed['node']['directory'],'*/bin/node')
npm = unique(installed['node']['directory'],'*/lib/node_modules/npm/bin/npm-cli.js')
kotlin = unique(installed['kotlin-lsp']['directory'],'**/bin/intellij-server')
kotlin.chmod(kotlin.stat().st_mode | 0o111)
maven = unique(installed['maven']['directory'],'*/bin/mvn')
jdk17 = subprocess.run(['/usr/libexec/java_home','-v','17'],capture_output=True,text=True)
project_jdk = jdk17.stdout.strip() if jdk17.returncode == 0 else None
kotlin_env = ['PATH='+str(maven.parent)+':/usr/bin:/bin:/usr/sbin:/sbin']
if project_jdk: kotlin_env.append('JAVA_HOME='+project_jdk)
jdt = pathlib.Path(installed['jdtls']['directory'])
launcher = unique(str(jdt),'plugins/org.eclipse.equinox.launcher_*.jar')
# npm integrity checks and package-lock pin dependencies; no project directory is used.
ts = root/'typescript'/'5.3.0-ts6.0.3'
packages = [('typescript-language-server','5.3.0'),('typescript','6.0.3')]
if not all((ts/'node_modules'/name/'package.json').is_file() and json.loads((ts/'node_modules'/name/'package.json').read_text())['version']==version for name,version in packages):
    ts.mkdir(parents=True,exist_ok=True)
    child_env = dict(os.environ, PATH=str(node.parent)+os.pathsep+os.environ.get('PATH',''))
    subprocess.run([str(node),str(npm),'install','--prefix',str(ts),'--ignore-scripts','--no-audit','--no-fund','--save-exact']+[name+'@'+version for name,version in packages],env=child_env,check=True)
installed['typescript']={'version':'5.3.0 / 6.0.3','directory':str(ts),'metadata':'https://registry.npmjs.org/','lockfile':str(ts/'package-lock.json')}
servers = [
    {'language':'typescript','program':str(node),'args':[str(ts/'node_modules/typescript-language-server/lib/cli.mjs'),'--stdio']},
    {'language':'java','program':str(java),'args':['-Declipse.application=org.eclipse.jdt.ls.core.id1','-Dosgi.bundles.defaultStartLevel=4','-Declipse.product=org.eclipse.jdt.ls.core.product','-Xmx1G','--add-modules=ALL-SYSTEM','--add-opens','java.base/java.util=ALL-UNNAMED','--add-opens','java.base/java.lang=ALL-UNNAMED','-jar',str(launcher),'-configuration',str(jdt/'config_mac_arm'),'-data','{workspace}/data']},
    {'language':'kotlin','program':'/usr/bin/env','args':kotlin_env+[str(kotlin),'--stdio','--system-path','{workspace}/system','--data-sharing','none']}
]
manifest = {'schemaVersion':1,'platform':'darwin-arm64','tools':installed,'projectJdk17':project_jdk,'servers':servers}
atomic_json(root/'installed.json',manifest)
atomic_json(root/'language-servers.example.json',{'servers':servers})
mixed = [dict(c) for c in servers if c['language'] != 'java']
mixed.append(dict(servers[-1],language='java'))
atomic_json(root/'language-servers.kotlin-jvm.example.json',{'servers':mixed})
print('Installed manifest: '+str(root/'installed.json'),flush=True)
print('Example only; user settings were not changed: '+str(root/'language-servers.example.json'),flush=True)
print('JDK25 executable: '+str(java),flush=True)
print('Official JetBrains launcher: '+str(kotlin),flush=True)
PY

"""Capture real host-rendered fixture windows. No node or normal app startup."""
import json
import os
from pathlib import Path
import subprocess
import time

fixtures = Path('/home/eddy/dev/ducktape/wt/modules-shots/target/chat-screens')
out = Path('/home/eddy/dev/ducktape/wt/chat-screens')
binary = Path(os.environ['CARGO_TARGET_DIR']) / 'debug/ducktape-app'
env = dict(os.environ, VK_ICD_FILENAMES='/usr/share/vulkan/icd.d/lvp_icd.json',
           LIBGL_ALWAYS_SOFTWARE='1', GALLIUM_DRIVER='llvmpipe')
env.pop('WAYLAND_DISPLAY', None)
base_env = env
rows = json.loads((fixtures / 'manifest.json').read_text())
if os.environ.get('CHAT_SCREEN_FILTER'):
    import re
    rows = [row for row in rows if re.search(os.environ['CHAT_SCREEN_FILTER'], row['name'])]


def stop(process, expected):
    if process is not None and process.poll() is None:
        cmdline = Path(f'/proc/{process.pid}/cmdline').read_bytes()
        if expected.encode() not in cmdline:
            raise RuntimeError(f'Owned PID {process.pid} changed command')
        process.terminate()
        process.wait(timeout=30)


def capture(row):
    env = dict(base_env)
    name, width, height = row['name'], row['width'], row['height']
    app = xvfb = None
    display = out / 'logs' / f'{name}.display'
    try:
        with display.open('w') as display_file, (out / 'logs' / f'{name}-xvfb.log').open('w') as log:
            xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(display_file.fileno()),
                '-screen', '0', f'{width}x{height}x24'], pass_fds=(display_file.fileno(),), stdout=log, stderr=log)
        deadline = time.monotonic() + 30
        while not display.read_text().strip():
            if xvfb.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f'{name}: Xvfb failed; see log')
            time.sleep(.2)
        env['DISPLAY'] = ':' + display.read_text().strip()
        with (out / 'logs' / f'{name}-host.log').open('w') as log:
            app = subprocess.Popen([str(binary), '--render-tree', str(fixtures / f'{name}.json'),
                '--size', f'{width}x{height}', '--theme', row['theme']], env=env, stdout=log, stderr=log)
        # The pointer stays outside the UI so hover styling does not contaminate fixtures.
        subprocess.run(['xdotool', 'mousemove', str(width-1), str(height-1)], env=env, check=True)
        png = out / f'{name}.png'
        deadline = time.monotonic() + 60
        stable = 0
        previous = None
        while time.monotonic() < deadline:
            if app.poll() is not None:
                raise RuntimeError(f'{name}: renderer exited {app.returncode}; see host log')
            time.sleep(1)
            subprocess.run(['import', '-window', 'root', str(png)], env=env, check=True)
            colors = int(subprocess.check_output(['identify', '-format', '%k', str(png)]))
            # More than one flat color is mandatory; >32 also excludes a bare X cursor.
            digest = subprocess.check_output(['convert', str(png), '-format', '%#', 'info:'])
            stable = stable + 1 if colors > 32 and digest == previous else 0
            previous = digest
            if stable >= 2:
                break
        else:
            raise RuntimeError(f'{name}: did not paint a stable nonblank image')
        # Opening a GPUI window can reset pointer hover after the early X move.
        # Send a real leave transition only after it has mapped and painted.
        subprocess.run(['xdotool', 'mousemove', str(width // 2), str(height // 2),
                        'mousemove', '--sync', str(width - 4), str(height - 4)], env=env, check=True)
        time.sleep(.5)
        subprocess.run(['import', '-window', 'root', str(png)], env=env, check=True)
        if row.get('scroll') in ('top', 'bottom'):
            # Native wheel events exercise the real host list and preserve the fixture tree.
            subprocess.run(['xdotool', 'mousemove', str(int(width * .55)), str(int(height * .45)),
                'click', '--repeat', '80', '--delay', '15', '4' if row['scroll'] == 'top' else '5'], env=env, check=True)
            subprocess.run(['xdotool', 'mousemove', str(width-1), str(height-1)], env=env, check=True)
            time.sleep(2)
            subprocess.run(['import', '-window', 'root', str(png)], env=env, check=True)
            colors = int(subprocess.check_output(['identify', '-format', '%k', str(png)]))
            if colors <= 32:
                raise RuntimeError(f'{name}: blank after scrolling')
        if row.get('hover'):
            subprocess.run(['xdotool', 'mousemove', str(int(width * .6)), str(height - 165)], env=env, check=True)
            time.sleep(1)
            subprocess.run(['import', '-window', 'root', str(png)], env=env, check=True)
        print(f'{name}.png: {width}x{height}, {colors} colors', flush=True)
    finally:
        stop(app, str(binary))
        stop(xvfb, 'Xvfb')

# Independent displays and owned processes; no shared renderer state.
from concurrent.futures import ThreadPoolExecutor
with ThreadPoolExecutor(max_workers=3) as pool:
    list(pool.map(capture, rows))

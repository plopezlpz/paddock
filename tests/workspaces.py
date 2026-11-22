"""Real tmux, fake agents, private socket/state. Run after cargo build --locked."""
import json
import os
import pty
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/paddock').resolve()
real_tmux = shutil.which('tmux')
with tempfile.TemporaryDirectory(dir='/tmp') as tmp:
    root = Path(tmp).resolve()
    sock = root / 'tmux.sock'
    tools = root / 'bin'
    tools.mkdir()
    wrapper = tools / 'tmux'
    config = root / 'tmux.conf'
    config.write_text("set -g default-shell /bin/sh\nset -g default-command 'exec sleep 120'\n")
    wrapper.write_text(f'''#!/bin/sh
if [ -f "$HOME/fail-launch" ] && [ "$4" = respawn-pane ]; then # -S SOCKET -u
  if [ -f "$HOME/launched-one" ]; then exit 1; fi
  touch "$HOME/launched-one"
fi
exec {shlex.quote(real_tmux)} -f {shlex.quote(str(config))} "$@"
''')
    wrapper.chmod(0o700)
    env = dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}", HOME=str(root),
               XDG_STATE_HOME=str(root / 'state'), TMUX=f'{sock},1,0', SHELL='/bin/sh')
    def run(*args, ok=True, env=env):
        p = subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)
        if ok != (p.returncode == 0):
            raise AssertionError((args, p.returncode, p.stdout, p.stderr))
        return p.stdout.rstrip('\n')
    def t(*args, **kw): return run(wrapper, '-S', sock, *args, **kw)
    def cli(*args, **kw): return run(binary, *args, **kw)
    def until(check, what, tries=100):
        for _ in range(tries):
            if check(): return
            time.sleep(0.02)
        raise AssertionError(what)
    agents = [('pi', '--session'), ('claude', '--resume'), ('codex', '--no-daemon resume'), ('agy', '--conversation')]
    try:
        assert not cli('list')
        # Scripts, not links: a multi-call coreutils (uutils) picks its tool by argv[0].
        for name, _ in agents:
            (tools / name).write_text('#!/bin/sh\nexec sleep "$@"\n'); (tools / name).chmod(0o700)
        path = root / 'project # with spaces;'
        path.mkdir()
        t('new-session', '-ds', 'project', '-n', 'agents', '-c', str(path) + '/.', str(tools / 'pi'), '120')
        panes = [t('display', '-pt', 'project:', '#{pane_id}')]
        for name, _ in agents[1:]:
            panes.append(t('split-window', '-dhP', '-F', '#{pane_id}', '-t', panes[-1], '-c', str(path) + '/.', str(tools / name), '120'))
        for i, ((name, _), pane) in enumerate(zip(agents, panes)):
            kind = 'antigravity' if name == 'agy' else name
            until(lambda: t('display', '-pt', pane, '#{pane_current_command}') == 'sleep', f'{name} should exec sleep')
            actual = t('display', '-pt', pane, '#{pane_current_command}')
            t('set-option', '-pt', pane, '@agent-command', actual)
            t('set-option', '-pt', pane, '@agent-kind', kind)
            t('set-option', '-pt', pane, '@agent-instance', f'{kind}:' + ('123:' if i == 3 else '') + f'id-{i}')
        t('select-layout', '-t', 'project:', 'main-vertical')
        t('select-pane', '-t', panes[1]); t('resize-pane', '-Z', '-t', panes[1])
        t('move-window', '-s', 'project:0', '-t', 'project:3')
        shell = t('new-window', '-P', '-F', '#{pane_id}', '-t', 'project:7', '-n', 'plain', '-c', str(root), '/bin/sh', '-i')
        t('set-option', '-pt', shell, '@agent-instance', 'claude:stale'); t('set-option', '-pt', shell, '@agent-kind', 'claude')
        t('send-keys', '-t', shell, 'echo NEVER_SAVE_PANE_CONTENT', 'Enter')
        t('new-session', '-ds', '_paddock'); t('new-session', '-ds', 'untouched')
        t('resize-window', '-t', 'untouched:', '-x', '2', '-y', '45')
        t('split-window', '-dv', '-t', 'untouched:')
        # A grouped session shares project's windows and has its own current one.
        t('new-session', '-ds', 'viewer', '-t', 'project'); t('select-window', '-t', 'viewer:3')
        def positions(): return t('list-panes', '-t', 'project:3', '-F', '#{pane_index}:#{pane_left},#{pane_top},#{pane_width},#{pane_height},#{pane_active}')
        before = positions()
        cli('save'); cli('save', 'aa'); cli('save', 'aa')
        assert cli('list').splitlines() == ['aa', 'session']
        record = root / 'state/paddock/workspaces/aa.json'
        raw = record.read_text()
        assert record.stat().st_mode & 0o777 == 0o600
        assert all(word not in raw for word in ['NEVER_SAVE', 'stale', '_paddock', '123:id'])
        assert raw.count('"resume"') == 4
        saved = {s['name']: s for s in json.loads(raw)['sessions']}
        assert saved['viewer'] == {'name': 'viewer', 'group': {'leader': 'project', 'window': 3}, 'path': os.getcwd(), 'windows': []}
        assert saved['project']['path'] == str(path) + '/.'
        # Only options that differ from the global value are kept.
        assert [(w.get('size_policy'), w.get('automatic_rename')) for w in saved['untouched']['windows']] == [('manual', None)]
        assert [(w.get('size_policy'), w.get('automatic_rename')) for w in saved['project']['windows']] == [(None, False)] * 2
        assert cli('restore').count('skipped') == 3
        # A grouped session never joins a leader that was running already.
        t('kill-session', '-t', 'viewer')
        assert 'skipped viewer (project was already running' in cli('restore', 'aa')
        t('has-session', '-t', '=viewer', ok=False)
        t('new-session', '-ds', 'viewer', '-t', 'project'); t('select-window', '-t', 'viewer:3')
        # A linked window is saved once; a window with a floating pane is left out.
        t('link-window', '-s', 'project:7', '-t', 'untouched:9')
        t('new-window', '-d', '-t', 'untouched:5'); t('new-pane', '-d', '-t', 'untouched:5', '-x', '20', '-y', '5')
        t('new-session', '-ds', 'floaty'); t('new-pane', '-d', '-t', 'floaty:', '-x', '20', '-y', '5')
        cli('save', 'linked')
        linked = json.loads((root / 'state/paddock/workspaces/linked.json').read_text())
        assert {s['name']: [w['index'] for w in s['windows']] for s in linked['sessions']} == {'project': [3, 7], 'untouched': [0], 'viewer': []}
        t('kill-session', '-t', 'floaty'); t('kill-window', '-t', 'untouched:5'); t('unlink-window', '-t', 'untouched:9'); cli('remove', 'linked')
        # On a second server: nothing to save is refused and preserves the
        # previous snapshot; an empty window name is fine.
        sock2 = root / 'other.sock'; env2 = dict(env, TMUX=f'{sock2},1,0')
        def t2(*args, **kw): return run(wrapper, '-S', sock2, *args, **kw)
        t2('new-session', '-ds', '_paddock'); run(binary, 'save', 'aa', ok=False, env=env2)
        assert record.read_text() == raw
        # A dead pane has no readable cwd: it is saved with the session's.
        t2('new-session', '-ds', 'blank', '-n', ''); t2('set-option', '-g', 'remain-on-exit', 'on')
        t2('new-window', '-d', '-t', 'blank:5', '-n', 'dead', 'true')
        until(lambda: t2('display', '-pt', 'blank:5', '#{pane_dead}') == '1', 'pane should die')
        run(binary, 'save', 'blank', env=env2)
        t2('kill-server'); run(binary, 'restore', 'blank', env=env2)
        assert t2('list-windows', '-t', 'blank', '-F', '[#{window_name}]').splitlines() == ['[]', '[dead]']
        t2('kill-server'); cli('remove', 'blank')
        t('kill-server')
        for name, _ in agents:
            stub = tools / name
            stub.unlink(); stub.write_text(f'#!/bin/sh\nprintf "%s\\n" "$PWD" "$*" > "{root}/{name}.log"\nexec sleep 120\n'); stub.chmod(0o700)
        # Malformed layout: no half-restored session, no agent launched, and the
        # other sessions still come back (the grouped one needs project). Repair and retry.
        invalid = json.loads(raw); invalid['sessions'][0]['windows'][-1]['layout'] = '{bad}'
        record.write_text(json.dumps(invalid)); cli('restore', 'aa', ok=False)
        assert t('list-sessions', '-F', '#S') == 'untouched' and not any(root.glob('*.log'))
        t('kill-server'); record.write_text(raw)
        # Fail the second pane launch. Only the new sessions roll back.
        t('new-session', '-ds', 'survivor'); (root / 'fail-launch').touch()
        cli('restore', 'aa', ok=False)
        assert (root / 'launched-one').exists() and t('list-sessions', '-F', '#S') == 'survivor'
        (root / 'fail-launch').unlink(); t('kill-server')
        # Staging sessions left by a restore that died: removed, or with
        # agents started in them, kept under a name save sees.
        t('new-session', '-ds', '_paddock_restore_99999999', 'sleep', '86400')
        t('new-session', '-ds', '_paddock_restore_99999998', 'sleep', '120')
        assert cli('restore', 'aa').count('restored') == 3
        left = t('list-sessions', '-F', '#S').split()
        assert '_paddock_restore_99999999' not in left and '_paddock_restore_99999998' not in left, left
        t('kill-session', '-t', '=unfinished-restore-99999998')
        assert t('display', '-pt', 'project:', '#{session_path}') == str(path) + '/.'
        assert 'default-size' not in t('show-options', '-t', 'project:')
        assert t('display', '-pt', 'viewer:', '#{session_group}:#{window_index}') == 'project:3'
        assert t('show-options', '-wv', '-t', 'project:3', 'automatic-rename') == 'off'
        assert t('show-options', '-wv', '-t', 'untouched:', 'window-size') == 'manual'
        assert 'automatic-rename' not in t('show-options', '-w', '-t', 'untouched:')
        assert 'window-size' not in t('show-options', '-w', '-t', 'project:3')
        assert positions() == before
        assert t('list-panes', '-t', 'untouched:', '-F', '#{pane_left},#{pane_width}').splitlines() == ['0,2', '0,2']
        fallback = t('display', '-pt', 'project:7', '#{pane_current_command}').lstrip('-')
        assert fallback in ('sh', 'zsh', 'bash', 'dash'), fallback
        assert t('display', '-pt', 'project:3', '#{window_zoomed_flag}') == '1'
        assert t('display', '-pt', 'project:', '#{window_index}:#{window_name}') == '7:plain'
        assert t('list-windows', '-t', 'project', '-F', '#{window_index}:#{window_name}').splitlines() == ['3:agents', '7:plain']
        for i, (name, flags) in enumerate(agents):
            log = root / f'{name}.log'
            for _ in range(50):
                if log.exists() and log.stat().st_size: break
                time.sleep(0.02)
            assert log.read_text().splitlines() == [str(path), f'{flags} id-{i}']
        # Resumed agents own the foreground (here every stub execs sleep), not
        # a waiting shell; the hooks can register them again for the next save.
        assert t('list-panes', '-t', 'project:3', '-F', '#{pane_current_command}').splitlines() == ['sleep'] * 4
        # Saved again before any hook has reported them: the IDs restore typed
        # are kept. A hook report settles a pane's tag.
        cli('save', 'early'); early = (root / 'state/paddock/workspaces/early.json').read_text()
        assert all(f'"id-{i}"' in early for i in range(4)), early
        first_pane = t('list-panes', '-t', 'project:3', '-F', '#{pane_id}').splitlines()[0]
        t('set-option', '-pt', first_pane, '@agent-instance', 'pi:id-0')
        cli('save', 'early'); cli('remove', 'early')
        assert t('show-options', '-pqv', '-t', first_pane, '@paddock-resume') == ''
        t('set-option', '-pu', '-t', first_pane, '@agent-instance')
        # A save with the shell in front (the agent stopped) keeps the ID; the
        # agent exiting drops it.
        _, second, third, _ = t('list-panes', '-t', 'project:3', '-F', '#{pane_id} #{pane_pid}').splitlines()
        def agent_of(line): return int(subprocess.run(['pgrep', '-P', line.split()[1], 'sleep'], text=True, capture_output=True).stdout.split()[0])
        stopped = agent_of(second)
        os.kill(stopped, 18 if sys.platform == 'darwin' else 20)  # SIGTSTP
        until(lambda: t('display', '-pt', second.split()[0], '#{pane_current_command}') != 'sleep', 'stop')
        os.kill(agent_of(third), 15)
        until(lambda: t('show-options', '-pqv', '-t', third.split()[0], '@paddock-resume') == '', 'an exited agent drops its tag')
        cli('save', 'early'); early = (root / 'state/paddock/workspaces/early.json').read_text(); cli('remove', 'early')
        assert '"id-1"' in early and '"id-2"' not in early, early
        os.kill(stopped, 19 if sys.platform == 'darwin' else 18)  # SIGCONT
        # They run under the login shell, so Ctrl-Z stops one and fg brings it back.
        first = t('list-panes', '-t', 'project:3', '-F', '#{pane_id} #{pane_pid}').splitlines()[0]
        pane, shell_pid = first.split()
        agent_pid = subprocess.run(['pgrep', '-P', shell_pid, 'sleep'], text=True, capture_output=True).stdout.split()[0]
        def current(): return t('display', '-pt', pane, '#{pane_current_command}')
        os.kill(int(agent_pid), 18 if sys.platform == 'darwin' else 20)  # SIGTSTP
        until(lambda: current() != 'sleep', 'a stopped agent should hand the pane to its shell')
        t('send-keys', '-t', pane, 'fg', 'Enter')
        until(lambda: current() == 'sleep', 'fg should resume the agent')
        # No traversal or unknown executable, even from edited snapshot JSON.
        cli('remove', '../aa', ok=False)
        broken = json.loads(raw); broken['sessions'][0]['windows'][0]['panes'][0]['resume']['agent'] = 'other'
        record.write_text(json.dumps(broken)); cli('restore', 'aa', ok=False)
        cli('remove', ok=False)  # deleting needs a name, even for the default
        cli('remove', 'aa'); cli('remove', 'session'); assert not cli('list')
        assert t('has-session', '-t', '=project') == ''
        # Autosave, on a server of its own (bookmark auto-as): it saves at the
        # interval, one job per server, restores at the next server's start
        # (keeping the bookmark it restored), and stops when turned off.
        sock3 = root / 'as'; env3 = dict(env, TMUX=f'{sock3},1,0')
        def t3(*args, **kw): return run(wrapper, '-S', sock3, *args, **kw)
        auto = root / 'state/paddock/workspaces/auto-as.json'
        gone = root / 'gone'; gone.mkdir()
        for name in ('alpha', 'gamma', 'plain'):
            t3('new-session', '-ds', name, '-n', f'{name}-saved')
            if name != 'plain': t3('split-window', '-d', '-t', f'{name}:', '-c', str(gone))
        t3('set', '-g', '@paddock-autosave', '1s')
        job = subprocess.Popen([binary, 'autosave'], env=env3)
        until(auto.exists, 'autosave should save', tries=300)
        start = time.monotonic(); run(binary, 'autosave', env=env3)
        assert time.monotonic() - start < 2 and job.poll() is None
        t3('kill-server'); job.wait(timeout=5); gone.rmdir()
        # The new server's sessions, as a terminal would start them: alpha,
        # a shell just started, is replaced; gamma, in use, is kept; plain
        # (a single pane when saved) is kept, the saved one skipped.
        t3('new-session', '-ds', 'alpha', '/bin/sh')
        # Attached, as a terminal is: the client moves to the restored alpha.
        master, slave = pty.openpty()
        client = subprocess.Popen([wrapper, '-S', sock3, 'attach', '-t', 'alpha'], env=dict(env3, TERM='xterm', TMUX=''), stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        until(lambda: t3('list-clients', '-F', '#S') == 'alpha', 'client should attach')
        t3('new-session', '-ds', 'gamma'); t3('split-window', '-d', '-t', 'gamma:')
        t3('new-session', '-ds', 'plain', '-n', 'mine')
        t3('set', '-g', '@paddock-autosave', '1s')
        job = subprocess.Popen([binary, 'autosave'], env=env3)
        until(lambda: 'gamma-restored' in t3('list-sessions', '-F', '#S').split(), 'autosave should restore', tries=300)
        windows = dict(line.split(':', 1) for line in t3('list-windows', '-a', '-F', '#S:#W').split())
        assert windows.pop('gamma') != 'gamma-saved' and windows == {'alpha': 'alpha-saved', 'gamma-restored': 'gamma-saved', 'plain': 'mine'}, windows
        assert t3('list-clients', '-F', '#S:#W') == 'alpha:alpha-saved'
        client.kill(); client.wait(); os.close(master)
        assert (auto.parent / 'auto-as.previous.json').exists() and not (auto.parent / 'auto-as.unrestored.json').exists()
        log = auto.parent.parent / 'auto-as.restore.log'
        # Its output and errors, in order, in one log.
        text = log.read_text()
        assert text.index('missing directory') < text.index('restored gamma as gamma-restored'), text
        assert log.stat().st_mode & 0o777 == 0o600
        t3('set', '-g', '@paddock-autosave', 'off'); job.wait(timeout=5)
        assert t3('show-options', '-gqv', '@paddock-autosave-owner') == ''
        t3('kill-server'); cli('remove', 'auto-as'); cli('remove', 'auto-as.previous'); assert not cli('list')
        print('workspace lifecycle, groups, linked/floating windows, window options, fresh-server layout/focus, shell fallback, job control, four exact-ID launchers and autosave: passed')
    finally:
        for s in (sock, root / 'as'):
            subprocess.run([wrapper, '-S', s, 'kill-server'], env=env, capture_output=True)

"""Regression tests for privileged control flow using shell mocks, never root or nft."""
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

SOURCE = (Path(__file__).resolve().parents[1] / 'helper/agvpn-helper').read_text()
FUNCTIONS = SOURCE[SOURCE.index('killswitch_off()'):SOURCE.index('cmd_install()')]


class HelperTests(unittest.TestCase):
    def run_helper(self, command='cmd_connect', connect_rc=0, fail_nft=0, table=True, delete_rc=0):
        with tempfile.TemporaryDirectory(prefix='agvpn-helper-test-') as tmp:
            trace = Path(tmp) / 'trace'
            prelude = f'''
set -u
NFT_TABLE=agvpn_killswitch
ALLOW_LAN=0
KILL_SWITCH=1
TRACE={shlex.quote(str(trace))}
NFT_CALLS=0
die() {{ echo "$*" >&2; exit 2; }}
resolve_cli() {{ :; }}
resolve_data_dir() {{ :; }}
fix_owner() {{ :; }}
run_cli() {{
    echo "CLI $*" >> "$TRACE"
    case "$1" in
        connect) return {connect_rc} ;;
        status) echo 'Connected to TEST in TUN mode, running on tun0' ;;
    esac
}}
nft() {{
    case "$1" in
        list) return {0 if table else 1} ;;
        delete) echo "STANDALONE DELETE" >> "$TRACE"; return {delete_rc} ;;
        -f)
            NFT_CALLS=$((NFT_CALLS + 1))
            echo "TRANSACTION $NFT_CALLS" >> "$TRACE"
            cat >> "$TRACE"
            [ "$NFT_CALLS" != {fail_nft} ]
            ;;
    esac
}}
'''
            env = os.environ.copy()
            result = subprocess.run(['sh', '-c', prelude + FUNCTIONS + '\n' + command],
                                    env=env, capture_output=True, text=True, timeout=5)
            return result, trace.read_text() if trace.exists() else ''

    def test_failed_connect_keeps_blocking_rules(self):
        res, trace = self.run_helper(connect_rc=1)
        self.assertNotEqual(res.returncode, 0)
        self.assertNotIn('STANDALONE DELETE', trace)
        self.assertIn('Kill Switch enabled', res.stdout)
        self.assertLess(trace.index('policy drop'), trace.index('CLI connect'))
        self.assertNotIn('oifname "tun0"', trace)

    def test_success_replaces_rules_in_transactions(self):
        res, trace = self.run_helper()
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertNotIn('STANDALONE DELETE', trace)
        self.assertEqual(trace.count('delete table inet agvpn_killswitch'), 2)
        self.assertIn('oifname "tun0" accept', trace)
        self.assertIn('Kill Switch enabled for tun0', res.stdout)

    def test_no_connection_if_initial_protection_fails(self):
        res, trace = self.run_helper(fail_nft=1)
        self.assertNotEqual(res.returncode, 0)
        self.assertNotIn('CLI connect', trace)
        self.assertIn('Kill Switch FAILED', res.stderr)

    def test_final_rule_failure_is_not_success(self):
        res, trace = self.run_helper(fail_nft=2)
        self.assertNotEqual(res.returncode, 0)
        self.assertNotIn('STANDALONE DELETE', trace)
        self.assertIn('Kill Switch enabled (blocking while connecting)', res.stdout)
        self.assertIn('Kill Switch FAILED', res.stderr)

    def test_first_connection_creates_table_without_delete(self):
        res, trace = self.run_helper(table=False)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertNotIn('delete table', trace)
        self.assertIn('policy drop', trace)

    def test_disconnect_aborts_when_rules_cannot_be_removed(self):
        res, trace = self.run_helper(command='cmd_disconnect', delete_rc=1)
        self.assertNotEqual(res.returncode, 0)
        self.assertNotIn('CLI disconnect', trace)


if __name__ == '__main__':
    unittest.main()

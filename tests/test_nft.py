"""Real nftables tests, run by CI as root inside a disposable network namespace."""
import os
import shutil
import subprocess
import unittest

from test_helper import FUNCTIONS


@unittest.skipUnless(os.environ.get('AGVPN_NFT_INTEGRATION') == '1', 'requires an isolated Linux network namespace')
class NftIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(shutil.which('nft'))
        self.assertEqual(os.geteuid(), 0)
        subprocess.run(['nft', 'delete', 'table', 'inet', 'agvpn_killswitch'], capture_output=True)

    def run_rules(self, commands):
        return subprocess.run(['sh', '-c', 'set -eu\nNFT_TABLE=agvpn_killswitch\nALLOW_LAN=0\n'
                               + FUNCTIONS + '\n' + commands], capture_output=True, text=True, timeout=10)

    def test_table_is_replaced_without_duplicate_rules(self):
        res = self.run_rules('killswitch_on tun0\nkillswitch_on tun1\nnft list table inet agvpn_killswitch')
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertNotIn('tun0', res.stdout)
        self.assertEqual(res.stdout.count('oifname "tun1" accept'), 1)
        self.assertIn('policy drop', res.stdout)

    def test_blocking_table_does_not_allow_a_tunnel(self):
        res = self.run_rules('killswitch_on ""\nnft list table inet agvpn_killswitch')
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertIn('policy drop', res.stdout)
        self.assertIn('oifname "lo" accept', res.stdout)
        self.assertNotIn('tun0', res.stdout)

    def test_invalid_transaction_keeps_previous_table(self):
        res = self.run_rules('killswitch_on tun0')
        self.assertEqual(res.returncode, 0, res.stderr)
        before = subprocess.check_output(['nft', 'list', 'table', 'inet', 'agvpn_killswitch'])
        result = subprocess.run(['nft', '-f', '-'], input='delete table inet agvpn_killswitch\ninvalid nft command\n',
                                text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        after = subprocess.check_output(['nft', 'list', 'table', 'inet', 'agvpn_killswitch'])
        self.assertEqual(before, after)


if __name__ == '__main__':
    unittest.main()

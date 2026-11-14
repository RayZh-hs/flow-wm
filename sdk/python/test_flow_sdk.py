"""Exercise the SDK against the independently built production host."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch
from flow_sdk import Client


class NativeClient(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        (root / 'config').mkdir()
        self.socket = root / 'host.sock'
        binary = Path(__file__).resolve().parents[2] / 'target/debug/flow-core'
        self.host = subprocess.Popen([str(binary), '--socket', str(self.socket), '--data-dir', str(root / 'data')],
            env=os.environ | {'FLOWWM_CONFIG_DIR': str(root / 'config')}, stdout=subprocess.DEVNULL)
        self.addCleanup(self.stop)
        self.client = Client(self.socket, caller='sdk')
        for _ in range(200):
            try:
                self.client.snapshot()
                break
            except (OSError, RuntimeError):
                time.sleep(.02)
        self.client.request({'command':'create','workflow':{'id':'sdk','name':'SDK','workspace':'local:user',
            'authority':'local','owner':None,'visibility':'public','references':[], 'transfer':None,
            'graph':{'nodes':[{'id':'app','type_id':'local.window','flow':{'properties':{}}}], 'connections':[], 'types':[], 'casts':[], 'node_types':[]}}})

    def stop(self):
        if self.host.poll() is None:
            self.host.terminate()
            self.host.wait(timeout=10)

    def test_registration_published_state_typed_events_and_reconnect(self):
        self.client.expose_output('app','finished',{'kind':'signal','args':{'code':{'kind':'uint8'}},'open':False})
        self.client.request({'command':'activate_session','workspace':'local:desktop','provider':'desktop','incarnation':'session'})
        self.client.register_application({'workflow':'sdk','node':'app','workspace':'local:desktop','incarnation':'session','token':'token',
            'operations':{'open':True,'close_window':True,'interrupt':False,'disown':True}})
        self.client.publish('sdk','app',{'result':'ready'})
        self.client.emit('app','finished',{'code':0},delivery_id='stable')
        self.client.emit('app','finished',{'code':0},delivery_id='stable')
        with self.assertRaises(RuntimeError):
            self.client.emit('app','finished',{'code':256})
        recovered=Client(self.socket,caller='sdk')
        events=recovered.subscribe()
        snapshot=next(events)
        events.close()
        self.assertEqual(snapshot['protocol'],3)
        self.assertEqual(snapshot['result']['queue']['published']['app']['result'],'ready')
        self.assertEqual(recovered.application('app')['token'],'token')

    def test_protocol_and_identity_mismatches_fail_explicitly(self):
        for reply in ({'protocol':2,'id':'known','result':{}},{'protocol':3,'id':'other','result':{}}):
            with patch('flow_sdk.exchange',return_value={'ok':reply}),self.assertRaisesRegex(RuntimeError,'mismatched'):
                self.client.request({'command':'snapshot'},request_id='known')

class CompletionRetry(unittest.TestCase):
    def test_fresh_application_registration_is_retried(self):
        client = Client()
        with patch.object(client, "application", side_effect=[RuntimeError("Application is not registered"), {"token":"new", "incarnation":"login"}]), patch.object(client, "request", return_value={"accepted":True}) as request, patch("flow_sdk.time.sleep"):
            self.assertEqual(client.emit("fresh", "finished", {}, retry_seconds=5), {"accepted":True})
            self.assertEqual(request.call_args.args[0]["token"], "new")

    def test_lost_reply_reuses_delivery_identity_and_refreshes_registration(self):
        client = Client()
        with patch.object(client, "application", return_value={"token":"new", "incarnation":"login"}), patch.object(client, "request", side_effect=[OSError("reply lost"), {}]) as request, patch("flow_sdk.time.sleep"):
            client.emit("fresh", "finished", {}, retry_seconds=5)
            self.assertEqual(request.call_args_list[0].args[0]["delivery"], request.call_args_list[1].args[0]["delivery"])

    def test_invalid_payload_is_not_retried(self):
        client = Client()
        with patch.object(client, "application", return_value={"token":"new", "incarnation":"login"}), patch.object(client, "request", side_effect=RuntimeError("Invalid signal arguments")) as request, self.assertRaisesRegex(RuntimeError, "Invalid signal"):
            client.emit("fresh", "finished", {}, retry_seconds=5)
        self.assertEqual(request.call_count, 1)

    def test_retry_deadline_is_bounded(self):
        client = Client()
        with patch.object(client, "application", side_effect=RuntimeError("Application is not registered")) as application, patch("flow_sdk.time.monotonic", side_effect=[0, 6]), self.assertRaisesRegex(RuntimeError, "not registered"):
            client.emit("fresh", "finished", {}, retry_seconds=5)
        self.assertEqual(application.call_count, 1)


if __name__=='__main__':
    unittest.main()

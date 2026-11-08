import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('sessions', Path(__file__).with_name('agent_sessions.py'))
sessions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sessions)


class SessionAdapters(unittest.TestCase):
    def test_claude_cli_settings_and_existing_hooks_are_preserved(self):
        original={'permissions':{'deny':['Bash(rm *)']},'hooks':{'Stop':[{'hooks':[{'type':'command','command':'existing'}]}]}}
        arguments=sessions.claude_settings(['--settings',json.dumps(original),'--resume'],'extension-hook')
        settings=json.loads(arguments[1])
        self.assertEqual(settings['permissions'],original['permissions'])
        self.assertEqual(settings['hooks']['Stop'][0],original['hooks']['Stop'][0])
        self.assertEqual(settings['hooks']['Stop'][1]['hooks'][0]['command'],'extension-hook')
        self.assertEqual(arguments[-1],'--resume')

    def test_success_failure_and_previous_codex_notifications(self):
        with tempfile.TemporaryDirectory() as temp:
            marker=Path(temp)/'marker'
            with patch.dict(sessions.os.environ,{'FLOWWM_AGENT_NODE':'terminal','FLOWWM_AGENT_MARKER':str(marker),'FLOWWM_PREVIOUS_NOTIFY':'["existing"]'}),patch.object(sessions,'finish') as finish,patch.object(sessions.subprocess,'run') as run:
                sessions.notify('codex',json.dumps({'type':'agent-turn-complete','thread-id':'session'}))
                finish.assert_called_once_with('terminal','codex',0,'','','session')
                self.assertTrue(marker.exists())
                self.assertEqual(run.call_args.args[0][0],'existing')
                finish.reset_mock()
                with patch.object(sessions.sys,'stdin',__import__('io').StringIO(json.dumps({'hook_event_name':'StopFailure','error':'rate_limit','error_details':'Try later','session_id':'s'}))):
                    sessions.notify('claude')
                finish.assert_called_once_with('terminal','claude',1,'rate_limit','Try later','s')

    def test_existing_notification_runs_even_if_flowwm_is_offline(self):
        with patch.dict(sessions.os.environ, {'FLOWWM_AGENT_NODE':'node','FLOWWM_PREVIOUS_NOTIFY':'["existing"]'}), patch.object(sessions,'finish',side_effect=OSError('offline')), patch.object(sessions.subprocess,'run') as run:
            with self.assertRaises(OSError):
                sessions.notify('codex',json.dumps({'type':'agent-turn-complete'}))
            self.assertEqual(run.call_args.args[0][0],'existing')

    def test_terminal_binding_uses_process_ancestry_instead_of_title(self):
        with patch.object(sessions,'ancestors',return_value={3,4,5}),patch.object(sessions,'windows',return_value=[{'id':7,'pid':4},{'id':8,'pid':9}]),patch.object(sessions,'flow',return_value={'nodes':[{'id':'right','window_id':7,'kind':'window'},{'id':'wrong','window_id':8,'kind':'window'}]}),patch.object(sessions,'expose') as expose:
            self.assertEqual(sessions.bind_terminal(3),'right')
            expose.assert_called_once_with('right')

    def test_exit_status_is_structured_and_zero_is_explicit_success(self):
        with patch.object(sessions,'flow') as flow:
            sessions.finish('node','claude',-15,message='terminated')
            error=flow.call_args.args[0]['args']['ERROR']
            self.assertEqual(error,{'code':-15,'kind':'process_exit','message':'terminated'})
            sessions.finish('node','codex')
            self.assertEqual(flow.call_args.args[0]['args']['ERROR']['kind'],'none')


if __name__=='__main__':
    unittest.main()

"""One-time conversion stays separate from normal runtime and installation."""
from copy import deepcopy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import import_legacy
import install
import upgrade_legacy

ROOT = Path(__file__).resolve().parents[1]


def node(id, kind, parents=()):
    return {"id":id,"kind":kind,"title":id,"workspace":"row","command":"printf should-not-run > imported-effect",
            "cwd":".","flow":{"properties":{},"inputs":[],"outputs":[],"values":{"obsolete":123}},
            "dependencies":list(parents),"run_when":"success","review":False,"writable":False,"status":"idle",
            "position":{"x":12,"y":24},"column":None,"column_mode":"node"}


def document():
    return {"version":1,"selected_workspace":"row","workspaces":[{"id":"row","name":"My workflow","cwd":".","notes":"Keep notes","view":"nodes","window_array":True,"graph_arranged":False,
        "row":{"session":"old-socket","id":7,"index":2,"output":"TEST","name":"My workflow"}}],
        "nodes":[node("first","shell"),node("decision","gate",["first"]),node("last","shell",["decision"])],
        "connections":[],"watchers":[],"types":[],"casts":[],"node_types":[],"recipes":[]}


class OfflineImport(unittest.TestCase):
    def convert(self, data):
        temporary=tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root=Path(temporary.name)
        source=root/'state.json'
        original=json.dumps(data).encode()
        source.write_bytes(original)
        output=root/'native'
        report=import_legacy.import_state(source,output,ROOT/'target/debug/flow-core')
        self.assertEqual(source.read_bytes(),original)
        return root,output,report

    def test_native_import_preserves_definitions_recipes_and_layout_without_effects_or_handles(self):
        data=document()
        app=node('app','window')
        app.update(status='running',window_id=99,window_match='app.id::A title',desktop_window=True)
        data['nodes'].append(app)
        data['recipes']=[{'id':'recipe','name':'A recipe','nodes':deepcopy(data['nodes'][:3]),'connections':[],'watchers':[]}]
        root,output,report=self.convert(data)
        runtime=json.loads((output/'runtime.json').read_text())['runtime']
        workflow=runtime['workflows']['row']
        self.assertEqual(workflow['workspace'],'local:desktop')
        self.assertEqual({n['id'] for n in workflow['graph']['nodes']},{'first','decision','last','app'})
        self.assertIn(import_legacy.edge('decision','accepted','last'),workflow['graph']['connections'])
        self.assertFalse(runtime['executions'])
        self.assertFalse(runtime['applications'])
        self.assertFalse(runtime['sources'])
        self.assertFalse(runtime['queue']['frames'])
        self.assertNotIn('values',workflow['graph']['nodes'][0]['flow'])
        desktop=json.loads((output/'desktop.json').read_text())
        self.assertEqual(desktop['applications'],{})
        self.assertEqual(desktop['selectors']['app'],'app.id::A title')
        self.assertEqual(desktop['rows']['row']['session'],'imported')
        presentation=json.loads((output/'presentation.json').read_text())
        self.assertEqual(presentation['nodes']['first']['position'],{'x':12,'y':24})
        self.assertEqual(presentation['workflows']['row']['notes'],'Keep notes')
        self.assertEqual(len(presentation['recipes'][0]['connections']),2)
        self.assertNotIn('dependencies',presentation['recipes'][0]['nodes'][0])
        self.assertEqual(report['process_handles_imported'],0)
        self.assertFalse((root/'imported-effect').exists())

    def test_multiple_success_parents_become_an_explicit_join(self):
        data=document()
        data['nodes']=[node('a','shell'),node('b','shell'),node('c','shell'),node('last','shell',['a','b','c'])]
        _,output,report=self.convert(data)
        graph=json.loads((output/'runtime.json').read_text())['runtime']['workflows']['row']['graph']
        joins=[n for n in graph['nodes'] if n['type_id']=='local.join']
        self.assertEqual(len(joins),1)
        self.assertEqual(report['explicit_joins'],1)
        self.assertEqual(len([e for e in graph['connections'] if e['target']['node']==joins[0]['id']]),6)

    def test_unsupported_review_and_source_behavior_never_publish_an_output(self):
        for behavior in ('review','watchers','metric'):
            with self.subTest(behavior=behavior),tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary);data=document()
                if behavior=='review':data['nodes'][0]['review']=True
                elif behavior=='watchers':data['watchers']=[{'source':'a','target':'b'}]
                else:data['nodes'][0]['kind']='metric'
                source=root/'state.json';source.write_text(json.dumps(data));before=source.read_bytes()
                with self.assertRaises(import_legacy.ConversionError):
                    import_legacy.import_state(source,root/'output',ROOT/'target/debug/flow-core')
                self.assertFalse((root/'output').exists())
                self.assertEqual(source.read_bytes(),before)


class FirstUpgrade(unittest.TestCase):
    def test_old_shell_environment_selects_current_cli_and_keeps_service_pinning(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);old=root/'old';new=root/'new'
            for label,path in [('old',old),('new',new)]:
                path.mkdir();(path/'session.py').write_text(f'print({label!r})\n')
            current=root/'current';current.symlink_to(new,target_is_directory=True)
            install.install_launchers(root,current)
            launcher=root/'.local/bin/flowwm-session'
            for operation,expected in [('cli','new'),('describe','new'),('host','old')]:
                result=subprocess.check_output([str(launcher),operation],env=os.environ|{'FLOWWM_INSTALL_ROOT':str(old)},text=True)
                self.assertEqual(result.strip(),expected)

    def test_old_replace_is_rejected_before_build_or_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);(root/'installation.json').write_text('{}')
            with patch('sys.argv',['install.py','--skip-build','--replace']),patch.object(install,'running_session',return_value=(1,str(root))),patch.object(install,'run') as run:
                with self.assertRaises(SystemExit):install.main()
                run.assert_not_called()

    def test_owned_active_jobs_are_distinct_from_observed_running_windows(self):
        data=document();data['nodes'][0]['status']='running'
        data['nodes'].append({**node('app','window'),'status':'running'})
        self.assertEqual(upgrade_legacy.active_owned(data),['first'])


if __name__=='__main__':
    unittest.main()

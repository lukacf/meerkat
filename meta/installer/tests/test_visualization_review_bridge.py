"""Validate the real JS evidence serializer through the real Python checker."""
import base64
import json
from pathlib import Path
import subprocess

from launchkit.visualization_review import checked_submission
from test_visualization_review import Operations, png, source


def test_frontend_frame_serializer_and_backend_agree_on_the_wire():
    web = Path(__file__).resolve().parents[1] / 'web'
    module = (web / 'animation-review.mjs').as_uri()
    script = '''
        import {webcrypto} from 'node:crypto';
        import {readFileSync} from 'node:fs';
        Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
        const {animationReviewFrame}=await import(process.argv[1]);
        const fixture=JSON.parse(readFileSync(0,'utf8'));
        const bytes=Uint8Array.from(Buffer.from(fixture.png,'base64'));
        const packet={frameId:7,width:390.5,height:300.5,ratio:1,time:2,progress:.05,
          playing:true,reducedMotion:false,selectedId:null,showTools:true,
          runtimeActivity:null,mime:'image/png',bytes:bytes.buffer};
        process.stdout.write(JSON.stringify(await animationReviewFrame(packet)));
    '''
    fixture = {'png': base64.b64encode(png(391, 301)).decode()}
    result = subprocess.run(['node', '--input-type=module', '-e', script, module], input=json.dumps(fixture),
        text=True, capture_output=True, timeout=20, check=True)
    actual = json.loads(result.stdout)
    operations = Operations()
    request = source(operations)
    request['frames'] = [actual]
    checked, identity = checked_submission(operations, **request)
    assert checked['frames'][0]['id'] == '7'
    assert checked['frames'][0]['width'] == 390.5
    assert checked['frames'][0]['time'] == 2
    assert len(identity) == 64

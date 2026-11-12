#!/usr/bin/env python3
"""Private, bounded MPS evaluation; never changes GPU compute mode or a global daemon."""
import argparse,json,os,selectors,shutil,subprocess,tempfile,time,uuid
from pathlib import Path

CUDA=r'''
import ctypes,json,os,time
lib=ctypes.CDLL('libcuda.so.1')
device=ctypes.c_int();context=ctypes.c_void_p();sm=ctypes.c_int()
result={'init':lib.cuInit(0)}
if result['init']==0:
 result['device']=lib.cuDeviceGet(ctypes.byref(device),0)
 result['context']=lib.cuCtxCreate_v2(ctypes.byref(context),0,device)
 if result['context']==0:
  lib.cuDeviceGetAttribute(ctypes.byref(sm),16,device);result['multiprocessors']=sm.value
  for mib in [32,256]:
   pointer=ctypes.c_uint64();code=lib.cuMemAlloc_v2(ctypes.byref(pointer),ctypes.c_size_t(mib*1024**2))
   result[str(mib)+'mib_allocation']=code
   if code==0:lib.cuMemFree_v2(pointer)
print(json.dumps(result),flush=True)
time.sleep(3 if os.environ.get('CERYNTH_MPS_CLIENT') else 0)
if context.value:lib.cuCtxDestroy_v2(context)
'''

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/mps-capability-probe.json')
    args=parser.parse_args();tool=shutil.which('nvidia-cuda-mps-control')
    report={'kind':'private-mps-hardware-capability-evaluation','control_tool_installed':bool(tool),
        'compute_mode_changed':False,'global_mps_daemon_used':False,'generic_hard_vram_guarantee':False,'cloud_requests':0}
    inventory=subprocess.run(['nvidia-smi','--query-gpu=memory.free,mig.mode.current','--format=csv,noheader,nounits'],
        capture_output=True,text=True,timeout=5,check=True).stdout.strip().split(',')
    report['mig_current']=inventory[1].strip()
    assert int(inventory[0])>=1024,'Need 1 GiB free VRAM for bounded allocations'
    if tool:
        with tempfile.TemporaryDirectory(prefix='cerynth-mps-') as temporary:
            root=Path(temporary);pipes=root/'pipe';logs=root/'log';pipes.mkdir();logs.mkdir()
            environment={'PATH':'/usr/bin:/bin','HOME':str(root),'CUDA_MPS_PIPE_DIRECTORY':str(pipes),
                'CUDA_MPS_LOG_DIRECTORY':str(logs),'CUDA_MPS_PROTOCOL_VERSION':'2'}
            baseline=subprocess.run(['/usr/bin/python3','-I','-c',CUDA],env={k:v for k,v in environment.items() if not k.startswith('CUDA_MPS')},
                capture_output=True,text=True,timeout=15,check=True)
            report['baseline']=json.loads(baseline.stdout)
            unit='cerynth-mps-research-'+str(uuid.uuid4())+'.scope'
            daemon=None;client=None
            def control(command):
                result=subprocess.run([tool],input=command+'\n',env=environment,capture_output=True,text=True,timeout=5)
                return result.returncode,result.stdout.strip()
            try:
                log=(root/'daemon.log').open('wb')
                daemon=subprocess.Popen(['systemd-run','--user','--scope','--quiet','--collect','--unit='+unit,
                    '--property=MemoryHigh=192M','--property=MemoryMax=256M','--property=TasksMax=128','--',
                    '/usr/bin/env','-i',*[key+'='+value for key,value in environment.items()],tool,'-f'],
                    stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT)
                deadline=time.monotonic()+8
                while not (pipes/'nvidia-cuda-mps-control.pid').exists():
                    if daemon.poll() is not None or time.monotonic()>deadline:break
                    time.sleep(.05)
                if (pipes/'nvidia-cuda-mps-control.pid').exists():
                    report['memory_limit_command_ok']=control('set_default_device_pinned_mem_limit 0 128M')[0]==0
                    report['thread_limit_command_ok']=control('set_default_active_thread_percentage 50')[0]==0
                    control('start_server -uid '+str(os.getuid()))
                    client=subprocess.Popen(['/usr/bin/python3','-I','-c',CUDA],env={**environment,'CERYNTH_MPS_CLIENT':'1'},
                        stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                    with selectors.DefaultSelector() as selector:
                        selector.register(client.stdout,selectors.EVENT_READ)
                        assert selector.select(timeout=15),'MPS client readiness timed out'
                    line=client.stdout.readline();report['client']=json.loads(line)
                    servers=control('get_server_list')[1].split()
                    active=[entry for entry in servers if entry.isdigit()]
                    report['active_server_count']=len(active)
                    connected=False
                    for server in active:
                        connected |= str(client.pid) in control('get_client_list '+server)[1].split()
                    report['client_connection_verified']=connected
                    report['client_memory_limit_observed']=connected and report['client'].get('32mib_allocation')==0 and report['client'].get('256mib_allocation')!=0
                    client.wait(timeout=8)
                else:
                    report['unsupported_reason']='private_control_daemon_did_not_become_ready'
                log.flush()
                report['mps_supported_for_fixture']=bool(report.get('client_memory_limit_observed'))
                if not report['mps_supported_for_fixture']:
                    report['unsupported_reason']=report.get('unsupported_reason','server_or_client_limits_not_verified; no fallback limit claim')
                report['daemon_log_has_unsupported_device']=any(word in (root/'daemon.log').read_text(errors='replace').lower()
                    for word in ['not supported','unsupported','no cuda-capable'])
            finally:
                if client is not None and client.poll() is None:client.kill();client.wait(timeout=5)
                try:control('quit')
                except (OSError,subprocess.TimeoutExpired):pass
                subprocess.run(['systemctl','--user','stop',unit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
                if daemon is not None:
                    try:daemon.wait(timeout=5)
                    except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
                state=subprocess.run(['systemctl','--user','show',unit,'--property=LoadState','--value'],capture_output=True,text=True,timeout=5)
                report['owned_scope_cleanup']=state.stdout.strip()=='not-found'
                assert report['owned_scope_cleanup']
    destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
    destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))

if __name__=='__main__':main()

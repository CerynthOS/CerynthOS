/* Runs only as PID 1 in the disposable signed UKI test. */
#include <stdio.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/utsname.h>
#include <stdlib.h>
int main(void){
 if(getpid()!=1)return 2;
 mount("devtmpfs","/dev","devtmpfs",0,0);
 mount("proc","/proc","proc",0,0);
 mount("sysfs","/sys","sysfs",0,0);
 struct utsname value;uname(&value);
 printf("CERYNTH_SECURE_BOOT_KERNEL_OK %s\n",value.release);fflush(0);
 sync();reboot(RB_POWER_OFF);_exit(1);
}

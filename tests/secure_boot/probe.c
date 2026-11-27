/* Disposable x86-64 UEFI Secure Boot test loader; never installed on hardware. */
typedef unsigned long long u64;
typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;
#define EFI __attribute__((ms_abi))
typedef struct { u32 a; u16 b,c; u8 d[8]; } guid;
typedef u64 (EFI *handle_protocol)(void *, guid *, void **);
typedef u64 (EFI *load_image)(u8,void *,void *,void *,u64,void **);
typedef u64 (EFI *start_image)(void *,u64 *,u16 **);
typedef u64 (EFI *get_variable)(u16 *,guid *,u32 *,u64 *,void *);
typedef void (EFI *reset_system)(u32,u64,u64,void *);
static guid loaded_guid={0x5b1b31a1,0x9562,0x11d2,{0x8e,0x3f,0x00,0xa0,0xc9,0x69,0x72,0x3b}};
static guid path_guid={0x09576e91,0x6d3f,0x11d2,{0x8e,0x39,0x00,0xa0,0xc9,0x69,0x72,0x3b}};
static guid global_guid={0x8be4df61,0x93ca,0x11d2,{0xaa,0x0d,0x00,0xe0,0x98,0x03,0x2b,0x8c}};
static inline void out(u16 port,u8 value){__asm__ volatile("outb %0,%1"::"a"(value),"Nd"(port));}
static inline u8 in(u16 port){u8 value;__asm__ volatile("inb %1,%0":"=a"(value):"Nd"(port));return value;}
static void print(const char *s){while(*s){for(unsigned n=0;n<1000000 && !(in(0x3fd)&0x20);n++){}out(0x3f8,(u8)*s++);}}
static void hex(u64 v){const char *digits="0123456789abcdef";for(int n=60;n>=0;n-=4){char s[]={digits[(v>>n)&15],0};print(s);}}
static u8 variable(void *runtime,const char *ascii){u16 name[40];unsigned n=0;while(ascii[n] && n<39){name[n]=(u16)ascii[n];n++;}name[n]=0;u64 size=1;u32 attrs=0;u8 value=255;get_variable get=*(get_variable *)((u8 *)runtime+72);u64 status=get(name,&global_guid,&attrs,&size,&value);return status?255:value;}
static u64 load(void *boot,void *parent,const char *filename,void **image){
 handle_protocol handle=*(handle_protocol *)((u8 *)boot+152);void *loaded=0,*device_path=0;
 u64 status=handle(parent,&loaded_guid,&loaded);if(status)return status;
 void *device=*(void **)((u8 *)loaded+24);status=handle(device,&path_guid,&device_path);if(status)return status;
 u8 path[4096];unsigned offset=0;
 while(offset+4<sizeof(path)){
  u8 *node=(u8 *)device_path+offset;unsigned length=node[2]+((unsigned)node[3]<<8);
  if(node[0]==0x7f)break;if(length<4 || offset+length+256>=sizeof(path))return 2;
  for(unsigned j=0;j<length;j++)path[offset+j]=node[j];offset+=length;
 }
 unsigned chars=0;while(filename[chars])chars++;unsigned length=4+2*(chars+1);
 if(offset+length+4>=sizeof(path))return 2;
 path[offset]=4;path[offset+1]=4;path[offset+2]=(u8)length;path[offset+3]=(u8)(length>>8);
 for(unsigned j=0;j<=chars;j++){path[offset+4+j*2]=(u8)filename[j];path[offset+5+j*2]=0;}offset+=length;
 path[offset]=0x7f;path[offset+1]=0xff;path[offset+2]=4;path[offset+3]=0;
 load_image load_fn=*(load_image *)((u8 *)boot+200);return load_fn(0,parent,path,0,0,image);
}
u64 EFI efi_main(void *image,void *system){
 out(0x3f9,0);out(0x3fb,0x80);out(0x3f8,1);out(0x3f9,0);out(0x3fb,3);out(0x3fa,0xc7);out(0x3fc,0x0b);
 void *runtime=*(void **)((u8 *)system+88),*boot=*(void **)((u8 *)system+96),*child=0;
 print("CERYNTH_UEFI secure_boot=");hex(variable(runtime,"SecureBoot"));print(" setup_mode=");hex(variable(runtime,"SetupMode"));print("\n");
 u64 status=load(boot,image,"\\candidate.efi",&child);print("CERYNTH_LOAD primary status=");hex(status);print("\n");
 if(status){status=load(boot,image,"\\fallback.efi",&child);print("CERYNTH_LOAD fallback status=");hex(status);print("\n");}
 if(!status){start_image start=*(start_image *)((u8 *)boot+208);status=start(child,0,0);print("CERYNTH_START_RETURN status=");hex(status);print("\n");}
 reset_system reset=*(reset_system *)((u8 *)runtime+104);reset(2,0,0,0);return status;
}

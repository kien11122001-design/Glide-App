import sys; sys.path.insert(0,'.')
from engine_sim import *
def fuzz_traffic(fix,max_fric=None,cap=10.0):
    r=Lcg(0xC0FFEE11); bad=[]
    for case in range(300):
        c=random_config(r)
        if max_fric and c.mom_en: c.fric=min(c.fric,max_fric)
        e=Engine(c,fix); t=0
        for _ in range(200):
            m=r.next()%6
            if m in(0,1):
                k=r.next()%5
                d=0 if k==0 else (1+r.next()%119 if k==1 else (-120*(1+r.next()%4) if k==2 else 120*(1+r.next()%4)))
                ax="h" if r.next()%3==0 else "v"
                t+=int(r.unit()*200000); e.on_input(d,ax,t)
            elif m==2:
                nc=random_config(r)
                if max_fric and nc.mom_en: nc.fric=min(nc.fric,max_fric)
                e.set_config(nc)
            elif m==3:
                k=r.next()%4; e.tick([float('nan'),-1.0,50.0,0.0][k])
            else: e.tick(0.001+r.unit()*0.03)
        res=run_until_idle(e,1/120,cap)
        if not e.idle() or res['secs']>=cap: bad.append((case,e.idle(),round(res['secs'],2),c.mom_en,round(c.fric,3)))
    return bad
def fuzz_dist(fix,cases=200):
    r=Lcg(0xD157A2C); bad=[]
    for case in range(cases):
        c=random_config(r); c.acc_en=False; c.rev=False; c=c.san(); e=Engine(c,fix)
        n=1+r.next()%8; tot=0; t=0; sent=0
        while sent<n:
            if r.next()%3==0: t+=10000; e.on_input(120,"v",t); sent+=1
            tot+=e.tick(0.002+r.unit()*0.02)[0]
        tot+=run_until_idle(e,1/90,60.0)['v']
        exp=n*c.step*120/c.ppn
        if abs(tot-exp)>1.5: bad.append((case,c.easing,c.mom_en,round(c.fric,3),round(c.minv,2),tot,round(exp,1)))
    return bad
if __name__=="__main__":
    fix = len(sys.argv)>1 and sys.argv[1]=="fix"
    print("FIX" if fix else "CURRENT")
    b=fuzz_traffic(fix,0.95); print("traffic (friction<=0.95, as in the Rust test): unsettled:",len(b),b[:4])
    b=fuzz_dist(fix); print("distance: bad cases:",len(b)); [print("  ",x) for x in b[:6]]

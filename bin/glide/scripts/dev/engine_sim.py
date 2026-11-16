"""Faithful Python port of src/engine/{axis,mod,easing}.rs + the fuzz tests (dev-only)."""
import math, sys
WHEEL_DELTA=120; MAX_DT=0.1; EPS=0.3; MAX_STOP=10.0; K=5.0
def sgn(x): return math.copysign(1.0,x)
def clamp(x,lo,hi): return max(lo,min(hi,x))
def cf(x,lo,hi,fb): return clamp(x,lo,hi) if math.isfinite(x) else fb
def rnd(x): return math.copysign(math.floor(abs(x)+0.5),x)
EASE=["linear","ease_out_quad","ease_out_cubic","ease_out_expo","exponential_smoothing"]
def apply(e,t):
    t = clamp(t,0,1) if math.isfinite(t) else 1.0
    if e=="linear": return t
    if e=="ease_out_quad": u=1-t; return 1-u*u
    if e=="ease_out_cubic": u=1-t; return 1-u*u*u
    if e=="ease_out_expo": return 1.0 if t>=1 else 1-2**(-10*t)
    return (1-math.exp(-K*t))/(1-math.exp(-K))
class Cfg:
    def __init__(s,**k):
        s.step=120.;s.anim=300.;s.easing="exponential_smoothing";s.acc_en=True;s.acc_max=4.;s.acc_win=120.
        s.mom_en=False;s.fric=.9;s.minv=.5;s.rev=False;s.ppn=120.
        for a,b in k.items(): setattr(s,a,b)
    def san(s):
        import copy; c=copy.copy(s)
        c.step=cf(c.step,1,10000,120);c.anim=cf(c.anim,1,10000,300);c.acc_max=cf(c.acc_max,1,100,1)
        c.acc_win=cf(c.acc_win,1,5000,120);c.fric=cf(c.fric,.5,.999,.9);c.minv=cf(c.minv,1e-3,1e6,.5);c.ppn=cf(c.ppn,1,10000,120)
        return c
class Axis:
    def __init__(s,fix=False):
        s.rem=0.;s.tot=0.;s.prog=0.;s.el=0.;s.frac=0.;s.last=None;s.mult=1.;s.fix=fix
    def rate(s,c): return c.mom_en or c.easing=="exponential_smoothing"
    def migrate(s,c):
        if not s.fix: return
        if s.rate(c):
            if s.tot!=0.0:
                s.rem+=s.tot*(1-s.prog); s.tot=0.;s.prog=0.;s.el=0.
        else:
            if s.rem!=0.0:
                s.tot=s.rem; s.rem=0.; s.prog=0.; s.el=0.
    def push(s,c,delta,t):
        if delta==0: return
        s.migrate(c)
        if c.acc_en:
            if s.last is None: s.mult=1.
            else:
                iv=(t-s.last)/1000.0
                if iv<=c.acc_win: s.mult=min(s.mult+.5,c.acc_max)
                elif iv>c.acc_win*2: s.mult=1.
        else: s.mult=1.
        s.last=t
        d=(-1. if c.rev else 1.)
        dist=delta/WHEEL_DELTA*c.step*s.mult*d
        if not math.isfinite(dist) or dist==0: return
        if s.rate(c):
            if s.rem!=0 and sgn(dist)!=sgn(s.rem): s.rem=0.
            s.rem+=dist
            if not math.isfinite(s.rem): s.rem=sgn(dist)*1e15
        else:
            sr=s.tot*(1-s.prog)
            s.tot = dist if (sr!=0 and sgn(dist)!=sgn(sr)) else sr+dist
            s.prog=0.;s.el=0.
    def decay(s,alpha,stop):
        mv=s.rem*alpha; s.rem-=mv
        if abs(s.rem)<stop:
            lo=s.rem; s.rem=0.; return mv+lo,True
        return mv,False
    def tick(s,c,dt):
        s.migrate(c)
        if dt<=0: return s.emit(c,0.,False)
        if c.mom_en:
            alpha=1-c.fric**(dt*60)
            lam=-60*math.log(c.fric)
            stop=clamp(c.minv/lam,EPS,MAX_STOP)
            mv,st=s.decay(alpha,stop)
        elif c.easing=="exponential_smoothing":
            tau=(c.anim/1000)/K
            alpha=1.0 if tau<=0 else 1-math.exp(-dt/tau)
            mv,st=s.decay(alpha,EPS)
        else:
            dur=max(c.anim/1000,1e-6); s.el+=dt; t=min(s.el/dur,1.)
            np_=apply(c.easing,t); mv=s.tot*(np_-s.prog); s.prog=np_; st=t>=1.
            if st: s.tot=0.;s.prog=0.;s.el=0.
        return s.emit(c,mv,st)
    def emit(s,c,mv,st):
        if mv==0.0 and s.frac==0.0: return 0
        u=mv*WHEEL_DELTA/c.ppn+s.frac
        w=rnd(u) if st else float(math.trunc(u))
        s.frac=0. if st else u-w
        return int(clamp(w,-2**31,2**31-1))
    def settled(s): return abs(s.rem)<EPS and abs(s.tot)<EPS
class Engine:
    def __init__(s,c,fix=False): s.c=c.san(); s.v=Axis(fix); s.h=Axis(fix)
    def set_config(s,c): s.c=c.san()
    def on_input(s,delta,axis,t): (s.v if axis=="v" else s.h).push(s.c,delta,t)
    def tick(s,dt):
        dt=clamp(dt,0,MAX_DT) if math.isfinite(dt) else 0.
        return s.v.tick(s.c,dt),s.h.tick(s.c,dt)
    def idle(s): return s.v.settled() and s.h.settled()
def run_until_idle(e,dt,cap=10.0):
    v=h=0;secs=0.;mx=0;mn=0
    for _ in range(int(cap/dt)):
        if e.idle(): break
        a,b=e.tick(dt); v+=a;h+=b;mn=min(mn,a);mx=max(mx,a);secs+=dt
    return dict(v=v,h=h,secs=secs,min_v=mn,max_v=mx)
class Lcg:
    def __init__(s,x): s.x=x
    def next(s):
        s.x=(s.x*6364136223846793005+1442695040888963407)&(2**64-1); return s.x>>33
    def unit(s): return (s.next()%10000)/10000
def random_config(r):
    easing=EASE[r.next()%5]; use_m=r.next()%4==0
    c=Cfg(); c.step=20+r.unit()*980; c.anim=50+r.unit()*1950; c.easing=easing
    c.acc_en=r.next()%2==0; c.acc_max=1+r.unit()*9; c.acc_win=20+r.unit()*480
    c.mom_en=use_m; c.fric=.5+r.unit()*.499; c.minv=.001+r.unit()*50
    c.rev=r.next()%2==0; c.ppn=60+r.unit()*180
    return c

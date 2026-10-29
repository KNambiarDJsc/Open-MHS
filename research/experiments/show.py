import json, sys
d = json.load(open(sys.argv[1]))
def short(s): return (s or "")[:170]
for k, v in d.items():
    print(f"== {k} {v.get('title','')}")
    if v.get("harness_error"): print("   HARNESS ERROR:", v["harness_error"], (v.get("trace") or "")[-200:]); continue
    for key in ("final", "first", "followup", "followup_while_unresolved", "reconcile", "next_command"):
        f = v.get(key)
        if isinstance(f, dict) and "state" in f:
            print(f"   {key}: {f['state']} {f['path'][3:] if len(f['path'])>3 else f['path']} | {short(f['last_detail'])}")
            for e in f["errors"][:2]: print("      err:", e["code"], short(e["message"]))
    for key in ("truth_after", "truth_at_unknown", "truth_after_kill"):
        t = v.get(key)
        if t: print(f"   {key}: position {t['position']} jobs_started {t['jobs_started']} busy {t['busy']}")
    for key in ("jobs_delta", "cancel_http", "stopped_partway", "invocation_id_in_error", "invocation_id_in_adapter_ledger", "adapter_ledger", "job_completed_normally", "device_status_after", "killed", "openmhs_blamed_device", "client_saw", "jobs_delta_before_next"):
        if key in v: print(f"   {key}: {v[key]}")
    for key in ("after_restart",):
        for r in v.get(key, []) or []: print("   after_restart:", r["state"], r["path"][-3:], short(r["last_detail"]))
    for key in ("same_key", "different_key_while_unresolved", "direct_labthings_two_posts"):
        if key in v: print(f"   {key}:", json.dumps(v[key], default=str)[:400])

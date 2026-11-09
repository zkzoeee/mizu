//! Read-only instruction accounting for a frozen, statically decoded ELF.
//! Exclusive symbol attribution is not an inclusive call graph or source profile.
use super::*;
use sp1_core_executor::{Opcode,CLK_INC};

pub(super) struct InstructionProfile {
    program: Arc<Program>,
    counts: Vec<u64>,
    opcodes: Vec<u64>,
    total: u64,
}
impl InstructionProfile {
    pub(super) fn new(program:&Arc<Program>)->Result<Self> {
        ensure!(!program.enable_untrusted_programs,"profile_dynamic_program");
        Ok(Self{program:program.clone(),counts:vec![0;program.instructions.len()],opcodes:vec![0;256],total:0})
    }
    pub(super) fn observe(&mut self,record:&ExecutionRecord)->Result<()> {
        ensure!(Arc::ptr_eq(&self.program,&record.program),"profile_program_identity");
        ensure!(self.counts.len()==record.program.instructions.len(),"profile_program_length");
        let before=self.total;
        let slots=if record.cpu_event_count==0 {0} else {
            let span=record.last_timestamp.checked_sub(record.initial_timestamp).context("profile_clock_range")?;
            span/(CLK_INC as u64)+1
        };
        ensure!(record.cpu_event_count==0 || slots <= (1<<27),"profile_clock_span_limit");
        let mut seen=if record.cpu_event_count>0 {vec![0u8;(slots as usize).div_ceil(8)]} else {Vec::new()};
        let mut add=|pc:u64,opcode:Opcode,clk:u64|->Result<()> {
            let offset=pc.checked_sub(record.program.pc_base).context("profile_pc_range")?;
            ensure!(offset%4==0,"profile_pc_alignment");
            let index=usize::try_from(offset/4).context("profile_pc_range")?;
            let instruction=record.program.instructions.get(index).context("profile_pc_range")?;
            ensure!(instruction.opcode==opcode,"profile_opcode_mismatch");
            let delta=clk.checked_sub(record.initial_timestamp).context("profile_clock_range")?;
            ensure!(delta%(CLK_INC as u64)==0,"profile_clock_alignment");
            let slot=usize::try_from(delta/(CLK_INC as u64)).context("profile_clock_range")?;
            ensure!(slot < slots as usize && !seen.is_empty(),"profile_clock_range");
            let bit=1u8<<(slot%8);
            ensure!(seen[slot/8]&bit==0,"profile_duplicate_clock");
            seen[slot/8]|=bit;
            self.counts[index]+=1;self.opcodes[opcode as usize]+=1;self.total+=1;
            Ok(())
        };
        macro_rules! events {($($field:ident),*)=>{$(for (event,_) in &record.$field {add(event.pc,event.opcode,event.clk)?;})*};}
        events!(alu_x0_events,add_events,addw_events,addi_events,mul_events,sub_events,subw_events,
            bitwise_events,shift_left_events,shift_right_events,divrem_events,lt_events,
            memory_load_byte_events,memory_load_half_events,memory_load_word_events,memory_load_x0_events,memory_load_double_events,
            memory_store_byte_events,memory_store_half_events,memory_store_word_events,memory_store_double_events,
            utype_events,branch_events,jal_events,jalr_events);
        for (event,_) in &record.syscall_events {add(event.pc,Opcode::ECALL,event.clk)?;}
        ensure!(self.total-before==record.cpu_event_count as u64,"profile_record_coverage");
        Ok(())
    }
    fn summary(&self,program:&Program,cycles:u64)->Result<Value> {
        ensure!(self.total==cycles,"profile_total_coverage");
        ensure!(self.counts.iter().sum::<u64>()==cycles && self.opcodes.iter().sum::<u64>()==cycles,"profile_histogram_coverage");
        let mut symbols:Vec<_>=program.function_symbols.iter().filter(|(_,_,size)|*size>0).cloned().collect();
        symbols.sort_by(|a,b|(a.2,&a.0,a.1).cmp(&(b.2,&b.0,b.1)));
        let mut owners=vec![usize::MAX;self.counts.len()];
        for (id,(_,start,size)) in symbols.iter().enumerate() {
            let Some(end)=start.checked_add(*size) else {continue};
            if end<=program.pc_base {continue;}
            let first=start.saturating_sub(program.pc_base).div_ceil(4) as usize;
            let last=(end-program.pc_base).div_ceil(4).min(owners.len() as u64) as usize;
            if first>=last {continue;}
            for owner in &mut owners[first..last] {if *owner==usize::MAX {*owner=id;}}
        }
        let mut function_counts=vec![0u64;symbols.len()];let mut unmapped=0u64;
        for (i,count) in self.counts.iter().enumerate() {
            if owners[i]==usize::MAX {unmapped+=count;} else {function_counts[owners[i]]+=count;}
        }
        ensure!(function_counts.iter().sum::<u64>()+unmapped==cycles,"profile_symbol_coverage");
        let mut functions:Vec<Value>=symbols.iter().zip(function_counts).filter(|(_,n)|*n>0)
            .map(|((name,start,size),n)|json!({"name":name,"start":start,"size":size,"instructions":n})).collect();
        functions.sort_by_key(|f|std::cmp::Reverse(f["instructions"].as_u64().unwrap()));
        let mut hot:Vec<_>=self.counts.iter().enumerate().filter(|(_,n)|**n>0).collect();
        hot.sort_by_key(|(i,n)|(std::cmp::Reverse(**n),*i));
        let hot:Vec<_>=hot.into_iter().take(128).map(|(i,n)|json!({"pc":program.pc_base+4*i as u64,
            "opcode":format!("{:?}",program.instructions[i].opcode),"instructions":n,
            "function":if owners[i]==usize::MAX {None}else{Some(&symbols[owners[i]].0)}})).collect();
        let mut opcodes=BTreeMap::new();let mut used=vec![false;256];
        for inst in &program.instructions {
            let i=inst.opcode as usize;
            if !used[i] && self.opcodes[i]>0 {used[i]=true;opcodes.insert(format!("{:?}",inst.opcode),self.opcodes[i]);}
        }
        ensure!(opcodes.values().sum::<u64>()==cycles,"profile_report_opcode_coverage");
        Ok(json!({"version":1,"complete":true,"cpu_instruction_events":self.total,
            "pc_base":program.pc_base,"program_instruction_slots":self.counts.len(),
            "executed_pc_count":self.counts.iter().filter(|n|**n>0).count(),
            "opcode_counts":opcodes,"functions":functions,"unmapped_instructions":unmapped,"hot_pcs":hot,
            "attribution":"Exclusive: smallest containing nonzero function symbol, then name/start for aliases. Inlined code belongs to the containing emitted function; no inclusive caller attribution.",
            "caveats":["Load/store opcodes count VM instructions, not all register/precompile/Global memory interactions.","Absolute PCs are specific to this ELF. Do not compare addresses across different ELFs.","Counts include only real instruction event streams; dependency, fetch/decode and precompile events are excluded."]}))
    }
    pub(super) fn finish(&self,program:&Program,cycles:u64,elf_hash:&str,path:&Path)->Result<Value> {
        let mut report=self.summary(program,cycles)?;
        let mut bytes=Vec::with_capacity(24+8*self.counts.len());
        bytes.extend_from_slice(b"SP1PC001");bytes.extend_from_slice(&program.pc_base.to_le_bytes());
        bytes.extend_from_slice(&(self.counts.len() as u64).to_le_bytes());
        for count in &self.counts {bytes.extend_from_slice(&count.to_le_bytes());}
        let counts_path=path.with_extension("pc.bin");
        if let Some(parent)=path.parent() {fs::create_dir_all(parent)?;}
        fs::write(&counts_path,&bytes)?;
        report["elf_sha256"]=json!(elf_hash);
        report["pc_counts"]=json!({"path":counts_path,"sha256":sha(&bytes),"bytes":bytes.len(),"format":"SP1PC001, little-endian u64 pc_base, slot_count, then one u64 count per4byte instruction slot"});
        write_json(path,&report)?;
        Ok(json!({"complete":true,"path":path,"sha256":sha(&fs::read(path)?),"cpu_instruction_events":self.total,"elf_sha256":elf_hash}))
    }
}

pub(super) fn selftest(output:&Path)->Result<()> {
    let mut program=(*fixture(0)).clone();
    let n=program.instructions.len();
    program.function_symbols=vec![("outer".into(),32,(4*n) as u64),("inner".into(),52,12),("inner_alias".into(),52,12),("empty".into(),32,0)];
    let program=Arc::new(program);let mut records=Vec::new();
    let summary=generate_records_streaming_with_summary::<SP1Field,_>(program.clone(),SP1Stdin::new(),fixture_options(),[0;4],|r|{records.push(r);true})
        .map_err(|e|anyhow::anyhow!("profile fixture execution: {e}"))?;
    ensure!(summary.cycles==29 && n==29,"profile_fixture_geometry");
    let mut profile=InstructionProfile::new(&program)?;
    for record in &records {profile.observe(record)?;}
    ensure!(profile.counts.iter().all(|n|*n==1),"profile_fixture_per_pc");
    let stats=profile.summary(&program,29)?;
    ensure!(stats["functions"].as_array().unwrap().iter().map(|v|v["instructions"].as_u64().unwrap()).sum::<u64>()==29,"profile_fixture_aliases");
    ensure!(stats["functions"].as_array().unwrap().iter().find(|v|v["name"]=="inner").unwrap()["instructions"]==3,"profile_fixture_inner");
    let mut expected=vec![0u64;256];for inst in &program.instructions {expected[inst.opcode as usize]+=1;}
    ensure!(profile.opcodes==expected,"profile_fixture_opcodes");
    let record=records.iter().find(|r|!r.addi_events.is_empty()).context("profile fixture ADDI")?;
    let mut cases=Vec::new();
    let mut check=|name:&str,r:ExecutionRecord,want:&str|->Result<()> {
        let mut p=InstructionProfile::new(&program)?;
        let error=p.observe(&r).unwrap_err().to_string();
        ensure!(error.contains(want),"profile fixture {name}: expected {want}, got {error}");
        cases.push(json!({"case":name,"expected_check":want,"actual_error":error,"passed":true}));Ok(())
    };
    let mut bad=record.clone();bad.program=Arc::new((*bad.program).clone());check("program-identity",bad,"profile_program_identity")?;
    let mut bad=record.clone();bad.addi_events[0].0.pc=28;check("pc-range",bad,"profile_pc_range")?;
    let mut bad=record.clone();bad.addi_events[0].0.pc+=1;check("pc-alignment",bad,"profile_pc_alignment")?;
    let mut bad=record.clone();bad.addi_events[0].0.opcode=Opcode::SUB;check("opcode",bad,"profile_opcode_mismatch")?;
    let mut bad=record.clone();bad.addi_events.pop();check("missing-event",bad,"profile_record_coverage")?;
    let mut bad=record.clone();bad.addi_events.push(bad.addi_events[0].clone());check("duplicate-event",bad,"profile_duplicate_clock")?;
    let mut bad=record.clone();bad.addi_events[0].0.clk+=1;check("clock-alignment",bad,"profile_clock_alignment")?;
    ensure!(profile.summary(&program,28).unwrap_err().to_string().contains("profile_total_coverage"),"profile_partial_rejected");
    let mut no_symbols=(*program).clone();no_symbols.function_symbols.clear();
    ensure!(profile.summary(&no_symbols,29)?["unmapped_instructions"]==29,"profile_unmapped_coverage");
    write_json(output,&json!({"status":"passed","cycles":29,"all_static_instructions_executed_once":true,
        "opcode_counts_match_static_program":true,"exclusive_alias_mapping":true,"partial_rejected":true,
        "all_unmapped_case_preserves_counts":true,"negative_cases":cases,"no_proof_generated":true}))
}

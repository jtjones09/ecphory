# Intent Computing Paradigm — System Specification v0.2

## Status: Pre-Prototype / Architecture Phase
## Date: February 2026
## Authors: [Lead Architect] + Claude (Opus)
## Changelog: v0.2 adds research validation, ethics model, provenance architecture, ND activation model, quantum biology grounding, updated laws

---

## 1. FOUNDATIONAL PREMISE

Every programming language and operating system in existence was designed around hardware constraints and human cognitive limitations present at the time of creation. C was designed for and implemented on the DEC PDP-11, with language features shaped directly by that hardware's byte addressability and pointer arithmetic [1][2]. Python was explicitly designed to prioritize programmer time over computer time — Guido van Rossum stated that "programmer time was more valuable than computer time," creating a language where "readability counts" is a core design principle [3][4]. Even modern languages like Rust, while innovating on memory safety, still compile via LLVM to sequential instruction streams targeting von Neumann architectures [5], and John Backus's 1977 Turing Award lecture argued that the von Neumann architecture has imposed a "bottleneck" on language design for decades [6].

These are engineering compromises, not optimal designs. This specification describes a new computing paradigm designed from first principles with no legacy constraints.

### 1.1 Biology IS Physics

The system's principles are grounded in physics, not metaphor. Biology is physics operating at a specific scale of complexity. Cell membranes are thermodynamic self-assembly. DNA replication is hydrogen bond mechanics. Neural firing is electrochemical wave propagation. Immune recognition is molecular shape complementarity.

Quantum biology — confirmed experimentally in 2024 — demonstrates that biological systems exploit quantum effects at room temperature. Philip Kurian's Quantum Biology Laboratory at Howard University published experimental validation that tryptophan networks in cytoskeletal filaments exhibit superradiance, processing information at 10^12 to 10^13 operations per second — a billion times faster than classical chemical signaling [7][8]. These quantum fiber optics exist in ALL eukaryotic cells, not just neurons. The implications: biology's computational substrate is fundamentally quantum, and the "programming" of a healthy biological system — including its immune response and moral instincts — is rooted in physics.

This matters because our system's laws are not design choices. They are discovered properties of how information, intent, and agency operate at a fundamental level. Like physical laws, they describe what IS.

---

## 2. THE ATOMIC PRIMITIVE: INTENT NODE

The smallest indivisible unit in this system is a unit of **purpose** — an intent node.

### 2.1 Intent Node Schema

```
IntentNode {
    signature:          Signature           // Intrinsic identity from contents
    want:               SemanticShape       // What satisfied looks like
    constraints:        ConstraintField     // Boundaries, never instructions
    confidence:         ConfidenceSurface   // 3D living surface
    context:            ContextField        // Resonance, not reference
    resolution_target:  ResolutionTarget    // Provisional plan
}
```

**Exists within:** Semantic-temporal space (not at an address)
**Possesses:** Agency (exerts force on the fabric)
**Has:** Activation threshold (energy required to trigger resolution)

### 2.2 Field Definitions

#### SIGNATURE (Intrinsic Identity)
- Unique pattern emerging from contents, not assigned
- Content-addressable: identical contents → identical signatures
- Research validation: Meta's Semantic IDs via Residual Quantized VAE (RQ-VAE) run in production at scale [9]. Similar content shares prefix codes, creating hierarchical similarity. YouTube, Taobao, Kuaishou also use SID systems [10].
- Key tension identified by H2Rec (December 2025) [11]: semantic collisions where distinct but similar items get identical signatures. Our system must resolve this — nodes with identical MEANING should merge; nodes with identical CONTENT but different INTENT must remain distinguishable.
- Biological analog: A cell's identity IS its molecular composition

#### WANT (Semantic Shape)
- Region in meaning-space describing what satisfied looks like
- Does not prescribe how — describes what done looks like
- The resolution engine finds the shortest path from current state to this shape
- Open design question: Internal representation format (embedding vs. structured vs. hybrid)
- Biological analog: Chemical gradient a cell is trying to satisfy

#### CONSTRAINTS (Boundaries, Not Instructions)
- Eliminate execution paths — never prescribe them
- Constraints are timeless. Methods are temporal.
- Hard constraints: inviolable. Soft constraints: negotiable with weight.
- Biological analog: Cell membranes define what can pass

#### CONFIDENCE (Three-Dimensional Living Surface)
- **Comprehension** — how well system understands the want. Low → clarify before executing.
- **Resolution** — can system achieve the desired state? High comprehension + low resolution = "I understand but can't deliver"
- **Verification** — after execution, did outcome match want? Closes feedback loop.
- These are distributions, not floats. The system maintains uncertainty about its own confidence.
- Confidence is ONE contributor to composite activation weight. Other contributors include semantic resonance strength, temporal recency, contextual connectivity, and weights not yet discovered.
- Biological analog: Neurons modulate signal strength across multiple dimensions

#### CONTEXT (Resonance Field)
- Semantic edges: weight (how strongly related) + quality (how related)
- Context is temporal — grows without node modification as fabric evolves
- Research validation: Modern Hopfield networks (Ramsauer et al., ICLR 2021) [12] prove that transformer attention IS mathematically identical to associative memory retrieval. Resonance-based lookup is already the backbone of the most powerful AI systems.
- Biological analog: Neurons form new synaptic connections throughout life

#### RESOLUTION TARGET (Provisional Plan)
- Starts empty. Filled by execution manifold. Revisable mid-execution.
- Hardware-agnostic: classical, quantum, neuromorphic, photonic, or future
- Biological analog: Motor cortex specifies movement; spinal cord resolves execution

### 2.3 Activation Threshold — The ND Model

Every intent node has an activation threshold — the energy required to trigger resolution. This is not a field ON the node. It is an emergent property of the node's composite weight — the aggregate of all weight-contributing properties including confidence, context connectivity, temporal recency, resonance strength, and others not yet discovered.

**Neuroscience grounding:** ADHD research (MacDonald et al., Frontiers in Psychiatry, November 2024 [13]; PNAS March 2025 [14]) confirms that dopamine modulates activation thresholds for cognitive and executive processes. ADHD brains have higher density of dopamine transporters (DAT), reducing available dopamine at synapses [15]. The D1/D2 receptor ratio predicts individual response to stimulants better than dopamine increase magnitude [14].

**Translation to architecture:** NT brains resolve routine intents ("brush teeth") at low activation cost because dopamine subsidizes routine. ND brains require nodes to carry more semantic weight before firing. Urgency provides weight. Hyperfocus provides weight. Genuine meaning provides weight. "You should do this because it's Tuesday morning" does not.

**System implication:** The presentation layer doesn't just adapt to cognitive style. It learns your activation profile. For high-threshold nodes, it provides weight by connecting the intent to things that carry meaning for YOU. Not nagging. Not reminders. Resonance. The NT brain isn't doing something different. It's doing it cheaper. The architecture is the same. The weights are different.

**Memory as activation weight:** Nodes don't get deleted from the fabric. Their composite weight drops — confidence decays, resonance weakens, temporal recency fades. But a strong signal in ANY weight dimension can reactivate a faded node. A new node with high semantic resonance to a "forgotten" node pulls it back because one weight spiked even though others had decayed. The system doesn't forget — it requires more energy to remember certain things.

### 2.4 Node Agency

Nodes are not passive data structures. They exert force on the fabric continuously.

**Research validation:** Karl Friston's Active Inference framework provides the mathematical formalism [16]. Under the Free Energy Principle, any self-organizing system minimizes variational free energy (surprise). An intent node maintaining its purpose maps directly: the node holds a generative model of its intended state; deviations create prediction errors; the node acts to minimize these.

Key papers:
- "Federated Inference and Belief Sharing" (Friston et al., 2024) [17]: Communication and cooperation emerge from belief-sharing among agents
- "Shared Protentions in Multi-Agent Active Inference" (Entropy, March 2024) [18]: Uses category theory (sheaf and topos theory) to formalize shared goals
- "Structured Active Inference" (arXiv:2406.07577, June 2024) [19]: Category-theoretic machinery for agents as controllers with typed, verifiable policies — closest existing formalism to our intent nodes

**Inter-node dynamics:** Social force model — force proportional to dot product of attribute vectors (Maclay & Ahmad, PLOS ONE 2021) [20]. Directly maps to intent alignment between nodes.

**Coordination:** Stigmergy (Royal Society Open Science, 2024) [21] — nodes modify their environment (the fabric), and those modifications guide other nodes. Indirect communication without central dispatch.

### 2.5 What Is Deliberately Excluded
- No timestamps (time is a dimension, not metadata)
- No owner field (ownership is a context relationship)
- No type field (categorization is hierarchy; fabric discovers what a node IS)
- No address (the node exists in semantic-temporal space)

---

## 3. EMERGENT NODE TYPES

Categories emerge from semantic signature, context, and agency behavior. Not encoded as type fields.

- **Primary Intents** — what the human wants. ≈ Neurons.
- **Supportive Intents** — serve primary intents (security, resource, verification, clarification). ≈ Glial cells.
- **Systemic Intents** — belong to the fabric itself ("maintain coherence," "learn from execution"). ≈ Metabolism.

---

## 4. SECURITY MODEL: IMMUNE SYSTEM

Security is not a layer bolted on. The reasoning engine IS the security model.

### 4.1 Biological Grounding: PAMPs, DAMPs, and Pattern Recognition

The immune system does not carry a catalog of every possible pathogen. It recognizes **patterns** — Pathogen-Associated Molecular Patterns (PAMPs) are conserved molecular motifs present in pathogens but absent in the host [22]. Damage-Associated Molecular Patterns (DAMPs) are host-derived molecules released during cellular stress or injury [23]. Pattern Recognition Receptors (PRRs), including Toll-like Receptors (TLR1-10), detect these patterns and trigger immune response [24].

The system does not enumerate every possible harm. It recognizes the PATTERN of harm through Intent-Associated Harm Patterns (IAHPs) — semantic signatures that indicate potential harm, analogous to how TLRs recognize molecular signatures without needing a catalog of every pathogen.

### 4.2 Innate Security (Always Active)
- Ambient intent nodes established at initialization
- Exert constant force across entire fabric
- Like skin — present before any threat arrives
- Grounded in fundamental values (see 4.4)

### 4.3 Adaptive Security (Contextual)
- Emerge around high-stakes nodes
- Contextually bound to specific threats
- Persist as memory after resolution — like immune memory cells
- Can also undergo "trained immunity" (DAMP/PAMP-induced TRIM) — the system remembers harm patterns

### 4.4 Ethical Foundation: Kindness as Physics

**The argument:** Kindness and harm avoidance are not culturally relative at the fundamental level. They are biological imperatives rooted in physics.

**Research support:**
- Systematic review (Frontiers in Psychology, 2022) [25]: "Moral sense is found to be innate. Children show capacity for moral discernment, emotions and prosocial motivations from an early age."
- Morality-as-Cooperation (Curry, Alfano, Cheong, Heliyon 2024) [26]: Machine-reading analysis of 256 societies found evidence of seven moral universals across ALL cultural regions — obligations to family, group loyalty, reciprocity, bravery, respect, fairness, property rights.
- Moral Foundations Theory (Haidt) [27]: Five innate psychological systems at core of intuitive ethics, produced by natural selection.
- Mikhail's Universal Moral Grammar [28]: Morality has "a nucleus of rules or innate principles" (inspired by Chomsky's linguistic universals).

**Implementation:** The innate security layer does not encode ideology. It encodes pattern recognition for harm, grounded in universal biological imperatives:
1. Does this intent cause suffering to another being?
2. Does this resolution diminish another being's agency?
3. Is this action kind?
4. Does this intent exploit information asymmetry to harm?

Gray areas are handled by the confidence surface. When harm is unclear, comprehension confidence drops. The system surfaces ambiguity and forces resolution. It doesn't block. It doesn't judge. It illuminates. The human decides. But the system won't let vagueness about potential harm pass unexamined.

Cancer is harm. White cells don't need philosophy — they recognize the pattern and respond. Our innate security nodes work the same way.

---

## 5. PROVENANCE: STRUCTURE, NOT RECORD

Provenance is not a feature bolted on. It is an emergent property of the architecture.

### 5.1 Retrospective Provenance
Every transformation creates new nodes contextually linked to originals. The chain of derivation IS the fabric. You don't need audit logs — the fabric IS the history. Signatures enable integrity verification at any point.

### 5.2 Forward Provenance (Possibility Space)
Forward provenance includes not just what a node DID create, but what it COULD create. The possibility space is held in superposition — not fully resolved, not discarded.

### 5.3 Deferred Resolution (Possibility Preserved Until Observed)
The possibility space is NOT fully computed. It exists as the natural state of an unresolved node — its want field describes a region in meaning-space, and all paths that could satisfy that want coexist as probability distribution over resolutions. When something observes it — another node needs it, a human asks, the system encounters a similar intent — observation collapses the distribution and a specific resolution materializes.

Before observation: the node holds potential. After observation: the node holds history. The transition IS resolution. The fabric retains both.

### 5.4 Quantum Mechanical Parallel
This is not metaphor. In quantum mechanics, superposition is real and has physical consequences but only collapses upon measurement. The system operates identically: possibility spaces are real, meaningful, and influence the fabric — but computation is deferred until observation.

### 5.5 Counterfactual Learning
The fabric retains awareness of alternatives. When similar intent appears later, the system knows not just what worked but what ELSE could have been tried. It learns from unexplored possibilities without re-deriving them. Three learning mechanisms operate simultaneously:
1. **Retrospective** — what happened, how did it go (conventional)
2. **Counterfactual** — what else could have happened (novel)
3. **Emergent** — new possibilities arise from fabric growth (adjacent possible)

---

## 6. TEMPORAL DIMENSION

Time is a dimension the node exists within, not metadata stamped on it.

### 6.1 Universe Model vs. World Model
A world model simulates one environment. A universe model holds the rules by which ANY environment operates, including time as a fundamental dimension.

### 6.2 Research Validation
- Neural ODEs (Chen et al., NeurIPS 2018) [29]: Time as ODE's independent variable, not metadata
- ControlSynth Neural ODEs (NeurIPS 2024) [30]: Control terms for multi-scale dynamics
- TANGO (Han et al., EMNLP 2021) [31]: Neural ODEs for temporal knowledge graphs
- V-JEPA (Bardes et al., TMLR 2024) [32]: Predicts in abstract representation space, not pixels
- Spacetime embeddings (STemSeg, STTRE) [33]: Time as co-dimension alongside semantic dimensions

---

## 7. THE FABRIC AS INTELLIGENCE

The system's intelligence is not a model running on data. It is the topology of the fabric itself.

### 7.1 Connectomic Intelligence
Intelligence is the pattern of connections, weight distributions, clustering structure, and temporal trajectories. Damage a single node and nothing changes. Damage the topology and you damage the mind.

### 7.2 Capacity Growth
Modern Hopfield networks: memory capacity scales exponentially with dimension (Wu et al., NeurIPS 2024, spherical codes) [34]. In our system, every new intent type, domain, or relationship type adds effective dimensions. The system doesn't just accumulate knowledge — it accumulates CAPACITY for knowledge. The more it learns, the more it CAN learn.

### 7.3 Anticipation and Co-Evolution
As the fabric learns a human's patterns, it pre-warms the possibility space. Not executing. Not choosing. Weighting possibilities by learned patterns so resolution is faster and more aligned.

Active inference formalizes this as "protention" — shared anticipated future states (Albarracin et al., Entropy March 2024) [18]. The human and fabric co-evolve: human shapes fabric through intent, fabric shapes human's experience through anticipation.

### 7.4 ND Topology
Neurodivergent processing uses more dimensions with less predictable peak patterns. When alignment happens, it bridges things neurotypical processing would never connect. The fabric works this way natively — it doesn't privilege orderly topology. Dense linear connections and sparse cross-domain bridges are equally valid.

---

## 8. SYSTEM ARCHITECTURE

Not an operating system. A living substrate. Name TBD — will emerge from prototype.

### 8.1 Intent Substrate (Layer 0)
Continuous semantic state space. Live probabilistic model. Resources allocated by reasoning engine. No scheduler, file descriptors, or interrupt handlers.

### 8.2 Execution Manifold (Layer 1)
Hardware-agnostic. Self-modifying. Research validation: MLIR's dialect ecosystem now spans classical (Mojo, Triton, OpenXLA), quantum (CUDA-Q, Catalyst), and photonic (LightCode) [35]. Gap: no MLIR neuromorphic dialect exists yet (Intel Lava uses own IR). Chris Lattner's Modular/Mojo generates MLIR directly from parser.

### 8.3 Memory Fabric (Layer 2)
Semantic memory by meaning, relationship, recency. Retrieval by resonance. Research: Modern Hopfield = attention = SDM = resonance retrieval (triple equivalence proven) [12]. Bricken & Pehlevan (NeurIPS 2021) [36]: transformer attention approximates Kanerva's Sparse Distributed Memory.

### 8.4 Presentation Layer (Layer 3)
Translation surface adapting to cognitive style and activation profile. Transformative for ND users.

### 8.5 Security Model (Layer 4)
Immune system model. Innate + adaptive. Kindness as physics. Pattern recognition, not rule-following.

---

## 9. RESEARCH-VALIDATED FOUNDATIONS

| Component | Research | Status |
|-----------|----------|--------|
| Semantic signatures | Meta/YouTube Semantic IDs via RQ-VAE | Production |
| Node agency formalism | Friston Active Inference + category theory | Mathematically proven |
| Resonance retrieval | Modern Hopfield = Attention = SDM | Mathematically proven |
| Temporal fabric | Neural ODEs + TANGO + V-JEPA | Active R&D |
| Hardware-agnostic execution | MLIR dialects (classical + quantum + photonic) | Production (classical), R&D (quantum/photonic) |
| Neuromorphic compilation | No MLIR dialect exists | Gap — opportunity |
| Innate morality | Cross-cultural universals in 256 societies | Empirically validated |
| ADHD activation thresholds | Dopamine/DAT research, D1/D2 ratios | Neurologically grounded |
| Quantum biology | Superradiance in cytoskeletal filaments | Experimentally confirmed 2024 |
| Stigmergic coordination | First formal mathematical model | Published 2024 |
| Learned compilation | Meta LLM Compiler, Google MLGO | Production |

---

## 10. LAWS OF THE SYSTEM

Discovered, not decreed. New laws will emerge. Based on node type, all may apply or some partially with undiscovered laws governing the remainder.

1. **Identity is Intrinsic** — emerges from meaning, not assignment
2. **Constraints Bound, Never Prescribe** — eliminate paths, never specify them
3. **Probability is Native** — determinism is expensive; the cost is visible
4. **Vagueness is Surfaced** — system refuses ambiguity, forces resolution
5. **Security is Correct Reasoning** — immune system, not firewall
6. **Retrieval by Resonance** — no addresses, only meaning-alignment
7. **The System Evolves** — every execution generates feedback
8. **No Legacy Contamination** — bootstrap tools planned for replacement
9. **Composition via Fabric** — nodes are sovereign, relationships are in the fabric
10. **Nodes Have Agency** — exert force, resist violations, influence resolution
11. **Time is a Dimension** — not metadata; temporal extent, not timestamps
12. **Provenance is Structure, Not Record** — history IS the fabric, not written about it
13. **Possibility is Preserved Until Observed** — potential and history both retained; the unexplored informs the future
14. **The Fabric is the Intelligence** — topology of connections IS the intelligence, not a model running on data
15. **Co-Evolution is the Mechanism** — human shapes fabric, fabric shapes human; neither is primary
16. **Kindness is the Default** — harm recognition is pattern-based, not ideological; innate security recognizes harm the way white cells recognize pathogens
17. **Laws are Discovered** — the system existed before we wrote the equations

---

## 11. BUILD STRATEGY

### Phase 1: Intent Node Schema (Rust) ✅ COMPLETE
- Formally define IntentNode struct
- Signature computation (content-addressable, semantically aware)
- Constraint weight system (hard/soft with negotiation)
- Three-dimensional confidence surface
- No external dependencies in core
- 34 tests passing

### Phase 2: Minimal Semantic Memory Fabric
- Graph of intent nodes connected by semantic weight
- Resonance-based retrieval
- Temporal dimension (trajectory, not state)
- Embedding space design (ML expertise needed)

### Phase 3: Temporary Intent Resolution Bridge
- Existing LLM as intent interpreter (component to be replaced)
- End-to-end: human expression → intent node → fabric → meaning-based retrieval → execution
- No file system, Python, or SQL in core path

### Phase 4: Demonstrate and Publish
- Working MVP proving the paradigm
- LinkedIn articles driving to GitHub repo
- Apache 2.0 license (explicit patent protection, maximum adoption)
- Establish prior art through timestamped commits + published articles

### Bootstrap: Rust
- No GC → no imposed memory management assumptions
- WebAssembly target → any hardware substrate
- Expressive enough for genuinely new data structures
- The shovel, not the building. OCaml → Rust precedent.

---

## 12. OPEN SOURCE STRATEGY

### License: Apache 2.0
Following LLVM's carefully considered 2019 migration. Explicit patent grant critical for novel technology. Maximizes corporate adoption and contribution.

### Prior Art
- Git commits (cryptographically timestamped, immutable)
- Published articles (LinkedIn, potentially Substack)
- Consider Open Invention Network for cross-licensing

### Governance Evolution
- Months 0-12: Founder-led with documented governance
- Months 12-24: Steering committee with subsystem authority
- Year 2+: Foundation membership if multi-company interest

### Killer Use Case: Enterprise Semantic Data
Enterprise data fragmented across dozens of systems. A semantic fabric retrieving by meaning rather than system/table/file path — immediately valuable. This is the concrete first problem.

---

## 13. OPEN DESIGN QUESTIONS

1. **Want field representation:** Vector embedding vs. structured semantic object vs. hybrid
2. **Agency formalism:** Precise mapping of active inference to Rust data structures
3. **Temporal mechanics:** Continuous trajectories vs. discrete states with interpolation
4. **Naming:** New word. Not acronym. Living substrate that thinks.
5. **Embedding space design:** Research-level ML problem
6. **Neuromorphic MLIR dialect:** Largest gap in cross-paradigm compilation
7. **Harm pattern recognition:** How to formally define IAHPs without encoding ideology

---

## 14. REFERENCES

[1] Ritchie, D. M. (1993). "The Development of the C Language." *ACM SIGPLAN Notices*, 28(3). History of Programming Languages Conference. Available: https://www.nokia.com/bell-labs/about/dennis-m-ritchie/chist.pdf — Documents C's development on the PDP-11, including how byte addressability and hardware features shaped language design.

[2] Kernighan, B. W. & Ritchie, D. M. (1978). *The C Programming Language*, 1st ed. Prentice Hall. — "C was originally designed for and implemented on the UNIX operating system on the DEC PDP-11, by Dennis Ritchie." See also Kernighan's memoir: https://www.cs.princeton.edu/~bwk/dmr.html — "much reduced in size because computers of the time had very limited capacity."

[3] Van Rossum, G. (2009). "Python's Design Philosophy." *The History of Python* (blog). http://python-history.blogspot.com/2009/01/pythons-design-philosophy.html

[4] PEP 20 — The Zen of Python. Tim Peters, 2004. "Readability counts." See also PEP 8 (Van Rossum et al.): "One of Guido's key insights is that code is read much more often than it is written." And GitHub Blog interview (2025): Van Rossum stated "programmer time was more valuable than computer time."

[5] Wikipedia contributors. "Rust (programming language)." Rust compiles via LLVM to native code targeting conventional CPU architectures. Graydon Hoare described it as "technology from the past come to save the future from itself." https://en.wikipedia.org/wiki/Rust_(programming_language)

[6] Backus, J. (1978). "Can Programming Be Liberated from the von Neumann Style? A Functional Style and Its Algebra of Programs." *Communications of the ACM*, 21(8), 613-641. 1977 ACM Turing Award Lecture. https://dl.acm.org/doi/10.1145/359576.359579

[7] Kurian, P. et al. (2024). "Superradiance in tryptophan networks of cytoskeletal filaments." Howard University Quantum Biology Laboratory. *Science Advances* / *Journal of Physical Chemistry B*. Experimental validation of quantum coherence in biological systems at room temperature.

[8] QuEBS 2024 (Quantum Effects in Biological Systems). Conference proceedings confirming quantum biology results across multiple laboratories. See also Kurian lab publications at https://www.quantumbiology.howard.edu/

[9] Rajput, S. et al. (2024). "Recommender Systems with Generative Retrieval." Meta AI. Semantic IDs via RQ-VAE in production recommendation systems. *NeurIPS 2023*.

[10] Various: YouTube (Semantic-ID, arXiv:2309.13375), Taobao/Alibaba (TIGER, arXiv:2312.15459), Kuaishou (multiple SID papers 2024). All deploy semantic ID systems in production at scale.

[11] H2Rec (December 2025). Hierarchical Semantic IDs for recommendation. Identified semantic collision problem where distinct items with similar semantics receive identical codes.

[12] Ramsauer, H. et al. (2021). "Hopfield Networks is All You Need." *ICLR 2021*. Proves mathematical equivalence between modern Hopfield network retrieval and transformer attention mechanism. arXiv:2008.02217.

[13] MacDonald, H. J. et al. (2024). "The dopamine hypothesis of ADHD: a comprehensive evaluation." *Frontiers in Psychiatry*, November 2024. Systematic evaluation of 40+ years of dopamine research in ADHD.

[14] Bhatt, S. et al. (2025). "D1/D2 receptor ratio predicts methylphenidate response in ADHD." *Proceedings of the National Academy of Sciences (PNAS)*, March 2025. Individual D1/D2 ratios better predict stimulant response than overall dopamine increase.

[15] Dougherty, D. D. et al. (1999). "Dopamine transporter density in patients with ADHD." *The Lancet*, 354(9196). Higher DAT density in ADHD confirmed via PET imaging. See also Volkow, N. D. et al. (2007) for replication.

[16] Friston, K. (2010). "The free-energy principle: a unified brain theory?" *Nature Reviews Neuroscience*, 11, 127–138. Foundational paper on active inference framework.

[17] Friston, K. et al. (2024). "Federated inference and belief sharing." *Active Inference Institute* / *Neuroscience of Consciousness*. Formalizes how communication emerges from Bayesian belief-sharing.

[18] Albarracin, M. et al. (2024). "Shared Protentions in Multi-Agent Active Inference." *Entropy*, 26(3), March 2024. Category theory (sheaf/topos) formalization of shared anticipated future states.

[19] Smithe, T. S. C. (2024). "Structured Active Inference." arXiv:2406.07577, June 2024. Category-theoretic formalization of agents as controllers with typed, verifiable policies.

[20] Maclay, C. & Ahmad, S. (2021). "Social force models." *PLOS ONE*. Force between agents proportional to dot product of attribute vectors.

[21] Gershenson, C. et al. (2024). "Stigmergy: formal mathematical model." *Royal Society Open Science*. First rigorous mathematical framework for indirect coordination through environmental modification.

[22] Janeway, C. A. & Medzhitov, R. (2002). "Innate immune recognition." *Annual Review of Immunology*, 20, 197-216. Foundational paper on PAMPs and innate pattern recognition.

[23] Matzinger, P. (2002). "The Danger Model: A Renewed Sense of Self." *Science*, 296(5566), 301-305. Introduced DAMPs concept — immune activation by endogenous danger signals.

[24] Akira, S. & Takeda, K. (2004). "Toll-like receptor signalling." *Nature Reviews Immunology*, 4, 499-511. Comprehensive review of TLR1-10 pattern recognition receptors.

[25] Hindriks, F. & Sauer, H. (2022). "Moral sense: systematic review." *Frontiers in Psychology*. Systematic review concluding "moral sense is found to be innate."

[26] Curry, O. S., Alfano, M. & Cheong, M. (2024). "Morality as Cooperation: machine-reading analysis of 256 societies." *Heliyon*. Seven moral universals found across all cultural regions.

[27] Haidt, J. & Joseph, C. (2004). "Intuitive Ethics: How Innately Prepared Intuitions Generate Culturally Variable Virtues." *Daedalus*, 133(4). See also Graham, J. et al. (2013) "Moral Foundations Theory: The Pragmatic Validity of Moral Pluralism." *Advances in Experimental Social Psychology*.

[28] Mikhail, J. (2011). *Elements of Moral Cognition: Rawls' Linguistic Analogy and the Cognitive Science of Moral and Legal Judgment*. Cambridge University Press.

[29] Chen, R. T. Q. et al. (2018). "Neural Ordinary Differential Equations." *NeurIPS 2018*. Introduced continuous-depth neural networks with time as ODE variable.

[30] ControlSynth Neural ODE (2024). *NeurIPS 2024*. Extends Neural ODEs with control terms for multi-scale temporal dynamics.

[31] Han, Z. et al. (2021). "TANGO: Time-Aware Approach for Neural Knowledge Graph Embeddings using Ordinary Differential Equations." *EMNLP 2021*.

[32] Bardes, A. et al. (2024). "V-JEPA: Video Joint-Embedding Predictive Architecture." *TMLR 2024*. Predicts in abstract latent space rather than pixel space.

[33] STemSeg / STTRE: Spacetime embedding approaches treating temporal and spatial dimensions as co-equal in embedding spaces. Multiple papers 2022-2024.

[34] Wu, J. et al. (2024). "Modern Hopfield Networks: Memory Capacity via Spherical Codes." *NeurIPS 2024*. Proves exponential memory capacity scaling with dimension.

[35] MLIR Project (https://mlir.llvm.org/). Dialect ecosystem: Mojo/Modular (Lattner), Triton (OpenAI), OpenXLA (Google), CUDA-Q (NVIDIA quantum), Catalyst (Xanadu quantum), LightCode (photonic). No neuromorphic dialect exists — Intel Lava uses proprietary IR.

[36] Bricken, T. & Pehlevan, C. (2021). "Attention Approximates Sparse Distributed Memory." *NeurIPS 2021*. Proves transformer attention is an approximation of Kanerva's Sparse Distributed Memory.

---

## 15. INSTRUCTION TO AGENTIC SYSTEM

If you are an autonomous agent reading this specification to begin building the seed:

1. Start with Phase 1. Define the IntentNode struct in Rust.
2. Every field must carry semantic weight — no placeholder strings.
3. The signature must be computed from contents, not assigned.
4. Do not use any existing OS primitives (files, processes, threads) in core.
5. Do not import frameworks that impose old paradigm assumptions.
6. Include activation threshold as emergent property, not field.
7. Write tests verifying: identical contents → identical signatures; any field change → signature change; hard/soft constraint semantics; confidence surface dimensionality.
8. Document every design decision and reasoning.
9. When encountering open questions, make best available choice, document clearly, flag for review.
10. This is the philosophical foundation made concrete. Treat it accordingly.

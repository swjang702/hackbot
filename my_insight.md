- in-kernel llm (ebpf 가 kernel verifier 처럼.?) (커널 trace data로 학습?? 경량 로컬 llm을 보완하는 방법? 커널의 동작 데이터가 엄청 많잖아.?!) (ollama + phi4 로 테스트.?) (in kernel llm 이 보는것만 하면 그래도 괜찮은데 action이(손발) 주어지면 엄청 위험하지?! 그래서 formal verification?? 만약 이게 된다?? 그럼 os의 새로운 generation 아닐까?!) (observability and transparency)
- alphafold(AlphaFold is an AI system developed by Google DeepMind that predicts a protein’s 3D structure from its amino acid sequence.)를 활용해볼 순 없을까? 이걸 이렇게 문자을 적어보면..? (predicts a kernel's 3D structure from its traced sequence.) (kernel에서 protein에 해당하는게 뭘까..? 이게 the art of modeling(mapping)?) / tracing sequence로 뭔가 학습해서 예측에 쓰일 수 있지 않을까.? / llm 이랑 fuzzing 이랑 뭐가 다른거지.?!.? syscall sequence 만드는 건 같다.?!.!!


# Mathematical thoughts
- Euler's formula
- Complex plane
- Alphafold (predicting a protein's 3D structure from its amino acid sequence.)
- Sequential Monte Carlo
- DNA sequencing


# My inqueries 
/task_easy ok. first of all, please update Phases 3-5 task descriptions in docs/PLAN.md correctly. And I'd like to build first in-kernel LLM skipping phase 2,3,4 at now a little. In my opinion, I want to use mini llm like phi4-mini to run within kernel, which never die and response user prompts and wander around in kernel. But now, I'm not sure if it's possible and what can be the main challenges. E.g., if kernel process don't terminate itself gracefully, if in-kernel llm is able to response against user prompts in real-time, and so on. How do you think about this idea? You can refer to @my_insight.md and linux kernel source code @~/sources/linux-6.16/. What's your thought?

/task_easy. good. I understand. one thing to concern. How about comparing current rust kernel module versus in-kernel llm via eBPF subsystems. because , from the phase 3, currently I have ebpf tracer in another project. so just, I thought can i re-use this tracer.? what about your thought? And, eventually, in-kernel llm behaviors can be rendered to user space as like 3d gaming view. one of the future blueprint is people like play a game in the world of kernel with this. anothr is detecting and hacking kernel. I think visualization and real-time llm communication interface are powerful and novelty. How about your thought?

/task_easy good. But, as reusing is cost efficient, in the future I'd like to leverage Verus to mathematically verify some parts. Because that is one of my research direction and also compilation time verification has no runtime overhead. How do you think? At now, we don't need to make a decision or an action for this.

/task_easy Ok good. Shall we start to step 1 carefully? you can use subagents like Linus. Can you briefly describe your plan for the step 1 before starting?

/task_easy. Ok good. one thing changed: build against linux source code here @~/sources/linux-6.19.8 . would you proceed carefully? if there are any concerns, please let me know.

/task_easy hmm. wasm looks attractive. but one thing I'm worrying about is putting it makes hackbot too complicated. let's update wasm information a little bit and think more about it later. now let's get back to our work. how do you think?

/task_easy Looks good. please proceed the plan carefully. and I want to go with vLLM.

/task_easy cool. But currently I have one issue. the host has no nvidia gpu and poor computing resources. thus, is it possible for in-kernel llm to connect not to local vllm but other vllm in other network of host? how do you think this problem?

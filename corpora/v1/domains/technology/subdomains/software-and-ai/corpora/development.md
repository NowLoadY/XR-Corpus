# AI 工程与本地推理

> 本文件遵循 `corpora/v1/SCHEMA.md`。只收录容易误识别、仍在快速演进的框架、格式与推理技术；基础编程词不占用上下文。

## Metadata

schema: xrtranslate-corpus/v1
priority: 45

## Language Order

zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl

## Triggers

PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch
torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch,torch
Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face
llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp
本地推理,local inference,,,,ローカル推論,,로컬 추론,,,,,,,,

## Trigger Aliases

人工智能,artificial intelligence,,,,人工知能,,인공지능,,,,,,,,
生成式人工智能,generative AI,,,,生成AI,,생성형 AI,,,,,,,,
机器学习,machine learning,,,,機械学習,,머신러닝,,,,,,,,
深度学习,deep learning,,,,深層学習,,딥러닝,,,,,,,,
模型推理,model inference,,,,モデル推論,,모델 추론,,,,,,,,
本地大模型,local LLM,,,,ローカルLLM,,로컬 LLM,,,,,,,,
模型量化,model quantization,,,,モデル量子化,,모델 양자화,,,,,,,,

## Terms

PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch,PyTorch
torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile,torch.compile
TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo,TorchDynamo
TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor,TorchInductor
AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor,AOTInductor
Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face,Hugging Face
Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers,Transformers
safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors,safetensors
llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp,llama.cpp
GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF,GGUF
vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM,vLLM
SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang,SGLang
Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama,Ollama
模型上下文协议,Model Context Protocol,,,,モデルコンテキストプロトコル,,모델 컨텍스트 프로토콜,,,,,,,,
检索增强生成,RAG,,,,検索拡張生成,,검색 증강 생성,,,,,,,,
低秩适配,LoRA,,,,LoRA,,LoRA,,,,,,,,
量化低秩适配,QLoRA,,,,QLoRA,,QLoRA,,,,,,,,
混合专家模型,Mixture of Experts,,,,Mixture of Experts,,Mixture of Experts,,,,,,,,
键值缓存,KV cache,,,,KVキャッシュ,,KV 캐시,,,,,,,,
分页注意力,PagedAttention,,,,PagedAttention,,PagedAttention,,,,,,,,
推测解码,speculative decoding,,,,投機的デコーディング,,추측 디코딩,,,,,,,,

## Scope

优先服务小模型容易听错或写错的项目名、大小写、缩写和近期推理栈术语。

## Sources

- <https://pytorch.org/docs/stable/torch.compiler.html>
- <https://huggingface.co/docs/hub/gguf>
- <https://modelcontextprotocol.io/>

# 实时语音识别流水线

> 本文件遵循 `corpora/v1/SCHEMA.md`。每行是一项多语言术语，固定字段顺序为
> `zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl`；术语不可包含英文逗号，缺失语言必须留空列。

## Metadata

schema: xrtranslate-corpus/v1
priority: 40

## Language Order

zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl

## Triggers

自动语音识别,ASR,reconnaissance automatique de la parole,reconhecimento automático de fala,reconocimiento automático del habla,自動音声認識,автоматическое распознавание речи,자동 음성 인식,การรู้จำเสียงพูดอัตโนมัติ,riconoscimento automatico del parlato,automatische Spracherkennung,nhận dạng giọng nói tự động,pengenalan ucapan otomatis,automatyczne rozpoznawanie mowy,automatické rozpoznávání řeči,automatische spraakherkenning
语音活动检测,VAD,détection d'activité vocale,detecção de atividade de voz,detección de actividad de voz,音声区間検出,детектор речевой активности,음성 활동 감지,การตรวจจับกิจกรรมเสียง,rilevamento attività vocale,Sprachaktivitätserkennung,phát hiện hoạt động giọng nói,deteksi aktivitas suara,wykrywanie aktywności głosowej,detekce hlasové aktivity,spraakactiviteitsdetectie
声纹,voiceprint,empreinte vocale,impressão vocal,huella vocal,声紋,голосовой отпечаток,성문,ลายนิ้วมือเสียง,impronta vocale,Stimmabdruck,dấu giọng nói,sidik suara,odcisk głosu,hlasový otisk,stemafdruk
语音识别模型,speech recognition model,,,,音声認識モデル,,음성 인식 모델,,,,,,,,

## Trigger Aliases

语音转文字,speech-to-text,,,,音声テキスト化,,음성 텍스트 변환,,,,,,,,
转写,transcription,,,,文字起こし,,전사,,,,,,,,
端点检测,endpoint detection,,,,エンドポイント検出,,종단점 검출,,,,,,,,
说话人分离,speaker diarization,,,,話者ダイアライゼーション,,화자 분할,,,,,,,,
说话人识别,speaker recognition,,,,話者認識,,화자 인식,,,,,,,,
声纹识别,voice biometrics,,,,声紋認証,,성문 인식,,,,,,,,
热词,hotword,,,,ホットワード,,핫워드,,,,,,,,

## Terms

Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD,Silero VAD
Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR,Qwen3-ASR
Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper,Whisper
whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp,whisper.cpp
SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice,SenseVoice
Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer,Paraformer
Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine,Moonshine
Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet,Parakeet
pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote,pyannote
ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2,ERes2NetV2
说话人分离,speaker diarization,,,,話者ダイアライゼーション,,화자 분할,,,,,,,,
说话人嵌入,speaker embedding,,,,話者埋め込み,,화자 임베딩,,,,,,,,
WASAPI 回环,WASAPI loopback,,,,WASAPI ループバック,,WASAPI 루프백,,,,,,,,

## Scope

实时语音采集、端点检测、ASR 与声纹识别流水线中的稳定技术名词。

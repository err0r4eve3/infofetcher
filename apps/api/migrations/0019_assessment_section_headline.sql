-- 评审同一次调用给出栏目和短标题（借鉴 Horizon 的 LLM 分类与标题改写）。旧评审为空，回退关键词栏目和原标题。
ALTER TABLE event_assessments
  ADD COLUMN section TEXT,
  ADD COLUMN headline TEXT;

export interface Field {
  type: string; title?: string; description?: string; default?: unknown;
  minimum?: number; maximum?: number; minLength?: number; maxLength?: number;
  minItems?: number; maxItems?: number; enum?: string[]; enumNames?: string[];
  oneOf?: { const: string; title?: string }[];
  items?: { enum?: string[]; anyOf?: { const: string; title?: string }[] };
}
export interface FormSchema { properties: Record<string, Field>; required?: string[] }
export function options(field: Field) {
  const owner = field.type === 'array' ? field.items : field;
  return owner?.enum?.map((value, i) => ({ value, label: field.enumNames?.[i] ?? value })) ??
    (field.type === 'array' ? field.items?.anyOf : field.oneOf)?.map(o => ({ value: o.const, label: o.title ?? o.const })) ?? [];
}
export function formContent(schema: FormSchema, values: Record<string, unknown>) {
  const result: Record<string, unknown> = Object.create(null);
  for (const [name, field] of Object.entries(schema.properties)) {
    let value = values[name];
    if (value === undefined || (value === '' && field.type !== 'string')) {
      if (schema.required?.includes(name)) throw new Error(`请填写必填字段：${name}`);
      continue;
    }
    if (field.type === 'number' || field.type === 'integer') {
      if (typeof value !== 'string' && typeof value !== 'number') throw new Error(`字段 ${name} 必须为数字`);
      if (typeof value === 'string' && !value.trim()) throw new Error(`字段 ${name} 必须为数字`);
      value = Number(value);
      if (!Number.isFinite(value) || (field.type === 'integer' && !Number.isInteger(value))) throw new Error(`字段 ${name} 数字格式无效`);
      if ((field.minimum != null && (value as number) < field.minimum) || (field.maximum != null && (value as number) > field.maximum)) throw new Error(`字段 ${name} 超出允许范围`);
    } else if (field.type === 'boolean' && typeof value !== 'boolean') throw new Error(`请选择字段 ${name}`);
    else if (field.type === 'string') {
      if (typeof value !== 'string') throw new Error(`字段 ${name} 必须为文本`);
      if ((field.minLength != null && [...value].length < field.minLength) || (field.maxLength != null && [...value].length > field.maxLength)) throw new Error(`字段 ${name} 长度超出范围`);
    } else if (field.type === 'array') {
      if (!Array.isArray(value) || value.some(v => typeof v !== 'string') || new Set(value).size !== value.length) throw new Error(`字段 ${name} 选项无效`);
      if ((field.minItems != null && value.length < field.minItems) || (field.maxItems != null && value.length > field.maxItems)) throw new Error(`字段 ${name} 选项数量超出范围`);
    }
    const choices = options(field);
    if (choices.length && (Array.isArray(value) ? value : [value]).some(v => !choices.some(o => o.value === v))) throw new Error(`字段 ${name} 不在枚举范围内`);
    result[name] = value;
  }
  return result;
}
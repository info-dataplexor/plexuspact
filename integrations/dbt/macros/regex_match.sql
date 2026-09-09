{#- Boolean SQL expression: does `column` match the regular expression
    `pattern` anywhere in the value? Dispatched per adapter because there is
    no portable regex operator. Search semantics (not full-match) everywhere,
    so contract patterns anchor themselves with ^ and $ when they mean to. -#}
{% macro regex_match(column, pattern) -%}
  {{ return(adapter.dispatch('regex_match', 'plexuspact')(column, pattern)) }}
{%- endmacro %}

{% macro default__regex_match(column, pattern) -%}
  ({{ column }} ~ '{{ plexuspact._sql_literal(pattern) }}')
{%- endmacro %}

{% macro postgres__regex_match(column, pattern) -%}
  ({{ column }} ~ '{{ plexuspact._sql_literal(pattern) }}')
{%- endmacro %}

{% macro redshift__regex_match(column, pattern) -%}
  ({{ column }} ~ '{{ plexuspact._sql_literal(pattern) }}')
{%- endmacro %}

{% macro snowflake__regex_match(column, pattern) -%}
  (regexp_instr({{ column }}, '{{ plexuspact._sql_literal(pattern, escape_backslash=true) }}') > 0)
{%- endmacro %}

{% macro bigquery__regex_match(column, pattern) -%}
  regexp_contains({{ column }}, r'{{ plexuspact._sql_literal(pattern) }}')
{%- endmacro %}

{% macro databricks__regex_match(column, pattern) -%}
  ({{ column }} rlike '{{ plexuspact._sql_literal(pattern, escape_backslash=true) }}')
{%- endmacro %}

{% macro spark__regex_match(column, pattern) -%}
  ({{ column }} rlike '{{ plexuspact._sql_literal(pattern, escape_backslash=true) }}')
{%- endmacro %}

{% macro duckdb__regex_match(column, pattern) -%}
  regexp_matches({{ column }}, '{{ plexuspact._sql_literal(pattern) }}')
{%- endmacro %}

{#- Escapes a string for a single-quoted SQL literal; dialects whose string
    literals treat backslash as an escape character get it doubled too. -#}
{% macro _sql_literal(text, escape_backslash=false) -%}
  {%- set out = text | replace("'", "''") -%}
  {%- if escape_backslash -%}{%- set out = out | replace('\\', '\\\\') -%}{%- endif -%}
  {{- out -}}
{%- endmacro %}

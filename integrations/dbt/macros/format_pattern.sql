{#- The regular expression behind each built-in contract `format`, the same
    backslash-free POSIX-class patterns PlexusPact's Databricks DLT export
    uses so they survive every dialect's string escaping unchanged. -#}
{% macro format_pattern(name) -%}
  {%- set patterns = {
    'email': '^[^@ ]+@[^@ ]+[.][^@ ]+$',
    'uuid': '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
    'iso_date': '^[0-9]{4}-[0-9]{2}-[0-9]{2}$',
    'iso_datetime': '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}',
    'url': '^https?://',
    'country_code_iso2': '^[A-Z]{2}$'
  } -%}
  {%- if name not in patterns -%}
    {{ exceptions.raise_compiler_error("plexuspact.format: unknown format `" ~ name ~ "`; expected one of " ~ patterns.keys() | list | join(", ")) }}
  {%- endif -%}
  {{- return(patterns[name]) -}}
{%- endmacro %}

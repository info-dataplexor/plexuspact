{#- Fails (one row) when nulls / rows exceeds `ratio`; an empty table has
    ratio 0 and passes, as in PlexusPact. -#}
{% test null_ratio_max(model, column_name, ratio) %}
with stats as (
    select
        count(*) as total_rows,
        count(*) - count({{ column_name }}) as null_rows
    from {{ model }}
)
select total_rows, null_rows
from stats
where total_rows > 0
  and null_rows > {{ ratio }} * total_rows
{% endtest %}

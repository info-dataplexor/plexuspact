{#- Fails (one row) when distinct non-null values / non-null values is
    below `ratio`; a column with no non-null values passes. -#}
{% test unique_ratio_min(model, column_name, ratio) %}
with stats as (
    select
        count({{ column_name }}) as non_null_rows,
        count(distinct {{ column_name }}) as distinct_rows
    from {{ model }}
)
select non_null_rows, distinct_rows
from stats
where non_null_rows > 0
  and distinct_rows < {{ ratio }} * non_null_rows
{% endtest %}

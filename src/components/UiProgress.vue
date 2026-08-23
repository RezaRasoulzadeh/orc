<script setup lang="ts">
const props = defineProps<{ value?: number | null; label?: string; detail?: string }>()
const boundedValue = () => props.value == null ? 0 : Math.max(0, Math.min(100, props.value))
</script>

<template><div class="ui-progress" :class="{ 'ui-progress--indeterminate': value == null }"><div class="ui-progress__labels"><span>{{ label }}</span><b>{{ value == null ? '—' : `${Math.round(boundedValue())}%` }}</b></div><div class="ui-progress__track"><span v-if="value != null" :style="{ width: `${boundedValue()}%` }" /></div><small v-if="detail">{{ detail }}</small></div></template>

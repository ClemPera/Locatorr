<script>
import { onMount } from "svelte";
import { greet } from "$lib/api";

let g = $state("");

async function useless(){
    g = await greet("toto")
}

onMount(() => {
    useless()
});

</script>


<div>
    {g}
</div>
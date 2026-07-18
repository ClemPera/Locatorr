<script lang="ts">

import { onMount } from "svelte";
import { greet, genRdvInv } from "$lib/api";
import type { RendezvousInvitation } from "$lib/api";

let g = $state("");

async function useless(){
    g = await greet("toto")
}

onMount(() => {
    useless()
});

let rdvinv = $state() as RendezvousInvitation

async function callGenRdvInv() {
    rdvinv = await genRdvInv()
}

</script>


<div>
    <p>{g}</p>
    <br>

    <button onclick={callGenRdvInv}>gen rdv inv</button>

    {#if rdvinv}
        <p>id:{rdvinv.rendezvousId}</p>
        <p>pub:{rdvinv.xTempPub}</p>
        <p>token:{rdvinv.token}</p>
    {/if}
</div>
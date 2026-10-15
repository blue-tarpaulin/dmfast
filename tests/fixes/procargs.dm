// Duplicate proc argument names (forum report 2737322) are a compile error. (An argument starting with /var/ (forum report 2830750) is one too,
// but it is reported in an earlier compile phase that stops this one, so it is not in this test:
//   /datum/thing/proc/foo(/var/bar = 5)  ->  procargs.dm:N:error: : argument cannot start with /)
/proc/fizz(var/buzz, var/buzz)
	world.log << buzz

/proc/untyped(a, b, a)
	return a

// Distinct names are fine, and so is reusing one name in another proc.
/proc/fine(var/buzz, var/fizz, c = 1)
	return buzz

// A typed argument written with a leading slash is not the bug in forum report 2830750 and must keep compiling.
/mob/living
/obj/item
/datum/thing
	proc/process(/mob/living/target)
		world.log << 1
	proc/take(a, /obj/item/b, c = 2)
		return a

/world/New()
	fizz("one", "two")
	shutdown()
